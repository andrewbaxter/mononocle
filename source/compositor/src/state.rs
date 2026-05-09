use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use mononocle_ipc::{WindowEvent, WindowInfo};
use smithay::{
    backend::renderer::{
        element::{
            Kind,
            render_elements,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            texture::{TextureBuffer, TextureRenderElement},
        },
        gles::{GlesRenderer, GlesTexture},
    },
    desktop::{LayerSurface as DesktopLayerSurface, PopupManager, Window, layer_map_for_output},
    input::{Seat, SeatState},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{
        Display, DisplayHandle,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::WlSurface,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Size, Transform},
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::{XdgShellState, XdgToplevelSurfaceData},
        },
        shm::ShmState,
    },
};

use crate::config::Config;
use crate::ipc_server::{IpcCommand, SharedIpcState};

render_elements! {
    pub CompElement<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Texture=TextureRenderElement<GlesTexture>,
}

static NEXT_WINDOW_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

pub fn next_window_id() -> u64 {
    NEXT_WINDOW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct ManagedWindow {
    pub id: u64,
    pub window: Window,
    pub desktop: u32,
}

impl ManagedWindow {
    pub fn title(&self) -> Option<String> {
        self.window.toplevel().and_then(|t| {
            with_states(t.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().ok())
                    .and_then(|g| g.title.clone())
            })
        })
    }

    pub fn app_id(&self) -> Option<String> {
        self.window.toplevel().and_then(|t| {
            with_states(t.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().ok())
                    .and_then(|g| g.app_id.clone())
            })
        })
    }

    pub fn to_info(&self, current_id: Option<u64>) -> WindowInfo {
        WindowInfo {
            id: self.id,
            title: self.title(),
            app_id: self.app_id(),
            desktop: self.desktop,
            is_visible: current_id == Some(self.id),
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct State {
    // --- Smithay protocol state ---
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,
    pub popup_manager: PopupManager,

    // --- Output ---
    pub output: Output,
    /// Logical size of the output (scale = 1).
    pub output_size: Size<i32, Logical>,

    // --- Window management ---
    pub windows: Vec<ManagedWindow>,
    pub current_window_id: Option<u64>,
    pub current_desktop: u32,
    pub layer_surfaces: Vec<DesktopLayerSurface>,

    // --- Background texture ---
    pub background_buffer: Option<TextureBuffer<GlesTexture>>,

    // --- IPC ---
    pub ipc_shared: Arc<Mutex<SharedIpcState>>,
    pub ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,

    // --- Misc ---
    pub config: Config,
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
}

impl State {
    pub fn new(
        display: &Display<Self>,
        output_size: Size<i32, Logical>,
        config: Config,
        ipc_shared: Arc<Mutex<SharedIpcState>>,
        ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    ) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let seat = seat_state.new_wl_seat(&dh, "seat0");
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let output = Output::new(
            "mononocle".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Mononocle".into(),
                model: "virtual".into(),
            },
        );
        let phys_size = Size::<i32, Physical>::from((output_size.w, output_size.h));
        let mode = Mode { size: phys_size, refresh: 60_000 };
        output.change_current_state(
            Some(mode),
            Some(Transform::Flipped180),
            None,
            Some(Point::from((0, 0))),
        );
        output.set_preferred(mode);
        output.create_global::<Self>(&dh);

        Self {
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
            popup_manager: PopupManager::default(),
            output,
            output_size,
            windows: Vec::new(),
            current_window_id: None,
            current_desktop: 0,
            layer_surfaces: Vec::new(),
            background_buffer: None,
            ipc_shared,
            ipc_rx,
            config,
            display_handle: dh,
            start_time: Instant::now(),
        }
    }

    /// Returns the screen area available after exclusive layer zones, with padding applied.
    pub fn window_area(&self) -> Rectangle<i32, Logical> {
        let layer_map = layer_map_for_output(&self.output);
        let zone = layer_map.non_exclusive_zone();
        let p = self.config.padding;
        Rectangle::new(
            Point::from((zone.loc.x + p, zone.loc.y + p)),
            Size::from(((zone.size.w - 2 * p).max(1), (zone.size.h - 2 * p).max(1))),
        )
    }

    pub fn show_window(&mut self, id: u64) {
        if self.current_window_id == Some(id) {
            return;
        }
        let prev = self.current_window_id;
        self.current_window_id = Some(id);

        if let Some(w) = self.windows.iter().find(|w| w.id == id) {
            self.current_desktop = w.desktop;
        }

        let area = self.window_area();
        if let Some(mw) = self.windows.iter().find(|w| w.id == id) {
            if let Some(toplevel) = mw.window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some(area.size);
                    state.states.set(xdg_toplevel::State::Activated);
                });
                toplevel.send_pending_configure();
            }
        }

        if let Some(prev_id) = prev {
            if let Some(mw) = self.windows.iter().find(|w| w.id == prev_id) {
                if let Some(toplevel) = mw.window.toplevel() {
                    toplevel.with_pending_state(|s| {
                        s.states.unset(xdg_toplevel::State::Activated);
                    });
                    toplevel.send_pending_configure();
                }
            }
        }

        self.push_event(WindowEvent::ShownWindowChanged { window_id: Some(id) });
        self.push_event(WindowEvent::ShownDesktopChanged { desktop: self.current_desktop });
    }

    pub fn show_desktop(&mut self, desktop: u32) {
        if self.current_desktop == desktop {
            return;
        }
        self.current_desktop = desktop;

        let first = self
            .windows
            .iter()
            .find(|w| w.desktop == desktop && w.window.alive())
            .map(|w| w.id);

        let prev = self.current_window_id;
        self.current_window_id = first;

        if let Some(prev_id) = prev {
            if let Some(mw) = self.windows.iter().find(|w| w.id == prev_id) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| { s.states.unset(xdg_toplevel::State::Activated); });
                    t.send_pending_configure();
                }
            }
        }

        if let Some(new_id) = first {
            let area = self.window_area();
            if let Some(mw) = self.windows.iter().find(|w| w.id == new_id) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = Some(area.size);
                        s.states.set(xdg_toplevel::State::Activated);
                    });
                    t.send_pending_configure();
                }
            }
        }

        self.push_event(WindowEvent::ShownDesktopChanged { desktop });
        self.push_event(WindowEvent::ShownWindowChanged { window_id: first });
    }

    pub fn kill_window(&mut self, id: Option<u64>) {
        let target = id.or(self.current_window_id);
        if let Some(wid) = target {
            if let Some(mw) = self.windows.iter().find(|w| w.id == wid) {
                if let Some(t) = mw.window.toplevel() {
                    t.send_close();
                }
            }
        }
    }

    pub fn process_pending(&mut self) {
        let dead: Vec<u64> = self
            .windows
            .iter()
            .filter(|w| !w.window.alive())
            .map(|w| w.id)
            .collect();
        for id in dead {
            self.remove_window(id);
        }

        self.layer_surfaces.retain(|s| s.alive());

        while let Ok(cmd) = self.ipc_rx.try_recv() {
            match cmd {
                IpcCommand::ShowDesktop(n) => self.show_desktop(n),
                IpcCommand::ShowWindow(id) => self.show_window(id),
                IpcCommand::KillWindow(id) => self.kill_window(id),
            }
        }
    }

    fn remove_window(&mut self, id: u64) {
        if self.current_window_id == Some(id) {
            let next = self
                .windows
                .iter()
                .filter(|w| w.id != id && w.desktop == self.current_desktop && w.window.alive())
                .map(|w| w.id)
                .next();
            self.current_window_id = next;
            if let Some(next_id) = next {
                let area = self.window_area();
                if let Some(mw) = self.windows.iter().find(|w| w.id == next_id) {
                    if let Some(t) = mw.window.toplevel() {
                        t.with_pending_state(|s| {
                            s.size = Some(area.size);
                            s.states.set(xdg_toplevel::State::Activated);
                        });
                        t.send_pending_configure();
                    }
                }
            }
            self.push_event(WindowEvent::ShownWindowChanged { window_id: self.current_window_id });
        }
        self.windows.retain(|w| w.id != id);
        self.push_event(WindowEvent::WindowDeleted { id });
        self.sync_ipc_windows();
    }

    pub fn push_event(&self, event: WindowEvent) {
        let mut shared = self.ipc_shared.lock().unwrap();
        for queue in shared.event_queues.iter_mut() {
            queue.push_back(event.clone());
        }
    }

    pub fn sync_ipc_windows(&self) {
        let windows: Vec<WindowInfo> = self
            .windows
            .iter()
            .filter(|w| w.window.alive())
            .map(|w| w.to_info(self.current_window_id))
            .collect();
        let mut shared = self.ipc_shared.lock().unwrap();
        shared.windows = windows;
        shared.current_window_id = self.current_window_id;
        shared.current_desktop = self.current_desktop;
    }

    pub fn render_elements(&self, renderer: &mut GlesRenderer) -> Vec<CompElement> {
        let mut elements: Vec<CompElement> = Vec::new();

        // Background image (rendered first, behind everything)
        if let Some(bg) = &self.background_buffer {
            let screen_logical = self.output_size;
            elements.push(
                TextureRenderElement::from_texture_buffer(
                    Point::from((0.0f64, 0.0f64)),
                    bg,
                    None,
                    None,
                    Some(screen_logical),
                    Kind::Unspecified,
                )
                .into(),
            );
        }

        let layer_map = layer_map_for_output(&self.output);

        // Layer shell: Background and Bottom (behind windows)
        for layer in [Layer::Background, Layer::Bottom] {
            for surface in layer_map.layers_on(layer) {
                let loc = layer_map
                    .layer_geometry(surface)
                    .map(|g| Point::<i32, Physical>::from((g.loc.x, g.loc.y)))
                    .unwrap_or_default();
                for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                    renderer, surface.wl_surface(), loc, 1.0, 1.0, Kind::Unspecified,
                ) {
                    elements.push(elem);
                }
            }
        }

        // Current window + its popups
        if let Some(id) = self.current_window_id {
            if let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) {
                if let Some(toplevel) = mw.window.toplevel() {
                    let area = self.window_area();
                    let win_loc = Point::<i32, Physical>::from((area.loc.x, area.loc.y));

                    for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                        renderer, toplevel.wl_surface(), win_loc, 1.0, 1.0, Kind::Unspecified,
                    ) {
                        elements.push(elem);
                    }

                    for (popup, popup_offset) in
                        PopupManager::popups_for_surface(toplevel.wl_surface())
                    {
                        let popup_loc = Point::<i32, Physical>::from((
                            area.loc.x + popup_offset.x,
                            area.loc.y + popup_offset.y,
                        ));
                        for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                            renderer, popup.wl_surface(), popup_loc, 1.0, 1.0, Kind::Unspecified,
                        ) {
                            elements.push(elem);
                        }
                    }
                }
            }
        }

        // Layer shell: Top and Overlay (in front of windows)
        for layer in [Layer::Top, Layer::Overlay] {
            for surface in layer_map.layers_on(layer) {
                let loc = layer_map
                    .layer_geometry(surface)
                    .map(|g| Point::<i32, Physical>::from((g.loc.x, g.loc.y)))
                    .unwrap_or_default();
                for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                    renderer, surface.wl_surface(), loc, 1.0, 1.0, Kind::Unspecified,
                ) {
                    elements.push(elem);
                }
            }
        }

        elements
    }

    pub fn send_frames(&self, time_ms: u32) {
        use smithay::wayland::compositor::{
            SurfaceAttributes, TraversalAction, with_surface_tree_downward,
        };

        let send_to = |surface: &WlSurface| {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, &()| TraversalAction::DoChildren(()),
                |_, states, &()| {
                    for cb in states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .frame_callbacks
                        .drain(..)
                    {
                        cb.done(time_ms);
                    }
                },
                |_, _, &()| true,
            );
        };

        if let Some(id) = self.current_window_id {
            if let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) {
                if let Some(t) = mw.window.toplevel() {
                    send_to(t.wl_surface());
                }
            }
        }

        let layer_map = layer_map_for_output(&self.output);
        for layer in [Layer::Background, Layer::Bottom, Layer::Top, Layer::Overlay] {
            for surface in layer_map.layers_on(layer) {
                send_to(surface.wl_surface());
            }
        }
    }
}
