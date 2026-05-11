use {
    crate::{
        compositor::{
            config::{
                BackgroundSize,
                Config,
                IdleHoldPolicy,
                WindowRule,
            },
            ipc_server::{
                IpcCommand,
                SharedIpcState,
            },
        },
        ipc::{
            WindowEvent,
            WindowInfo,
        },
    },
    regex::Regex,
    smithay::{
        backend::renderer::{
            Color32F,
            element::{
                Kind,
                render_elements,
                solid::{
                    SolidColorBuffer,
                    SolidColorRenderElement,
                },
                surface::{
                    WaylandSurfaceRenderElement,
                    render_elements_from_surface_tree,
                },
                texture::{
                    TextureBuffer,
                    TextureRenderElement,
                },
            },
            gles::{
                GlesPixelProgram,
                GlesRenderer,
                GlesTexture,
                Uniform,
                UniformValue,
                element::PixelShaderElement,
            },
        },
        desktop::{
            LayerSurface as DesktopLayerSurface,
            PopupManager,
            Window,
            layer_map_for_output,
        },
        input::{
            Seat,
            SeatState,
        },
        output::{
            Mode,
            Output,
            PhysicalProperties,
            Subpixel,
        },
        reexports::{
            wayland_protocols::xdg::shell::server::xdg_toplevel,
            wayland_server::{
                Display,
                DisplayHandle,
                backend::{
                    ClientData,
                    ClientId,
                    DisconnectReason,
                },
                protocol::wl_surface::WlSurface,
            },
        },
        utils::{
            IsAlive,
            Logical,
            Physical,
            Point,
            Rectangle,
            Size,
            Transform,
        },
        wayland::{
            compositor::{
                CompositorClientState,
                CompositorState,
                with_states,
            },
            idle_inhibit::IdleInhibitManagerState,
            output::OutputManagerState,
            selection::data_device::DataDeviceState,
            shell::{
                wlr_layer::{
                    Layer,
                    WlrLayerShellState,
                },
                xdg::{
                    XdgShellState,
                    XdgToplevelSurfaceData,
                },
            },
            shm::ShmState,
        },
    },
    std::{
        collections::HashSet,
        sync::{
            Arc,
            Mutex,
        },
        time::{
            Duration,
            Instant,
        },
    },
};

render_elements! {
    pub CompElement <= GlesRenderer>;
    Surface = WaylandSurfaceRenderElement < GlesRenderer >,
    Texture = TextureRenderElement < GlesTexture >,
    Solid = SolidColorRenderElement,
    PixelShader = PixelShaderElement,
}

static NEXT_WINDOW_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_window_id() -> u64 {
    NEXT_WINDOW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Current screen power state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPowerState {
    Active,
    Blanked,
    Off,
}

pub struct ManagedWindow {
    pub id: u64,
    pub window: Window,
    pub desktop: u32,
    pub fullscreen: bool,
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
    fn initialized(&self, _client_id: ClientId) { }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) { }
}

/// Effective decoration parameters after applying window rules.
pub struct EffectiveWindowParams {
    pub padding: i32,
    pub corner_radius: f32,
    pub inner_padding: i32,
    pub inner_padding_color: [f32; 4],
    pub border_thickness: i32,
    pub border_color: [f32; 4],
    pub fullscreen: bool,
    pub idle_hold: IdleHoldPolicy,
}

/// A compiled window rule with pre-built regexes.
struct CompiledRule {
    title_re: Option<Regex>,
    app_id_re: Option<Regex>,
    rule: WindowRule,
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
    pub idle_inhibit_state: IdleInhibitManagerState,
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
    // --- Background texture + original image dimensions (width, height) ---
    pub background_buffer: Option<(TextureBuffer<GlesTexture>, (u32, u32))>,
    // --- Rounded rect shader for decorations ---
    pub rounded_rect_shader: Option<GlesPixelProgram>,
    // --- IPC ---
    pub ipc_shared: Arc<Mutex<SharedIpcState>>,
    pub ipc_rx: std::sync::mpsc::Receiver<IpcCommand>,
    // --- Idle / screen power ---
    pub last_activity: Instant,
    pub last_mouse_activity: Instant,
    pub activity_mouse_pos: Point<f64, Logical>,
    pub screen_power_state: ScreenPowerState,
    pub cursor_visible: bool,
    /// Surfaces with active zwp_idle_inhibitor_v1 objects.
    pub idle_inhibit_surfaces: HashSet<WlSurface>,
    // --- Misc ---
    pub config: Config,
    compiled_rules: Vec<CompiledRule>,
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
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);
        let output = Output::new("mononocle".into(), PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Mononocle".into(),
            model: "virtual".into(),
        });
        let phys_size = Size::<i32, Physical>::from((output_size.w, output_size.h));
        let mode = Mode {
            size: phys_size,
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some(Point::from((0, 0))));
        output.set_preferred(mode);
        output.create_global::<Self>(&dh);

        let compiled_rules = compile_rules(&config.window_rules);
        let now = Instant::now();

        Self {
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            idle_inhibit_state,
            seat,
            popup_manager: PopupManager::default(),
            output,
            output_size,
            windows: Vec::new(),
            current_window_id: None,
            current_desktop: 0,
            layer_surfaces: Vec::new(),
            background_buffer: None,
            rounded_rect_shader: None,
            ipc_shared,
            ipc_rx,
            last_activity: now,
            last_mouse_activity: now,
            activity_mouse_pos: Point::from((0.0, 0.0)),
            screen_power_state: ScreenPowerState::Active,
            cursor_visible: true,
            idle_inhibit_surfaces: HashSet::new(),
            config,
            compiled_rules,
            display_handle: dh,
            start_time: now,
        }
    }

    // --- Idle / screen power ---

    /// Record user activity, resetting idle timers and waking the screen.
    pub fn record_activity(&mut self) {
        self.last_activity = Instant::now();
        if self.screen_power_state != ScreenPowerState::Active {
            self.screen_power_state = ScreenPowerState::Active;
            tracing::debug!("Screen woke up");
        }
    }

    /// Record mouse-based activity, applying the jitter threshold.
    /// Returns true if this counted as real activity.
    pub fn record_mouse_activity(&mut self, pos: Point<f64, Logical>) -> bool {
        let dx = pos.x - self.activity_mouse_pos.x;
        let dy = pos.y - self.activity_mouse_pos.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist >= self.config.mouse_jitter_threshold {
            self.activity_mouse_pos = pos;
            self.last_mouse_activity = Instant::now();
            if !self.cursor_visible {
                self.cursor_visible = true;
                tracing::debug!("Cursor shown (mouse moved)");
            }
            self.record_activity();
            true
        } else {
            false
        }
    }

    /// Check whether the current window effectively holds (inhibits) idle.
    pub fn is_idle_held(&self) -> bool {
        let Some(id) = self.current_window_id else { return false };
        let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) else { return false };
        let params = self.effective_window_params_for(mw);

        match params.idle_hold {
            IdleHoldPolicy::ForceHold => return true,
            IdleHoldPolicy::BlockHold => return false,
            IdleHoldPolicy::Default => {},
        }

        // Check fullscreen hold.
        if self.config.fullscreen_holds_idle && params.fullscreen {
            return true;
        }

        // Check wayland idle-inhibit protocol.
        if let Some(t) = mw.window.toplevel() {
            if self.idle_inhibit_surfaces.contains(t.wl_surface()) {
                return true;
            }
        }

        false
    }

    /// Check idle timeouts and transition screen power state.
    /// Called once per frame in the main loop.
    pub fn check_idle_timeouts(&mut self) {
        // Cursor hide uses mouse-only idle time and is independent of idle hold.
        if let Some(cursor_secs) = self.config.cursor_hide_timeout_secs {
            if self.cursor_visible && self.last_mouse_activity.elapsed() >= Duration::from_secs_f64(cursor_secs) {
                self.cursor_visible = false;
                tracing::debug!("Cursor hidden after {cursor_secs}s mouse idle");
            }
        }

        if self.is_idle_held() {
            return;
        }

        let elapsed = self.last_activity.elapsed();

        // Check display-off timeout first (it's the longer one).
        if let Some(off_secs) = self.config.display_off_timeout_secs {
            if elapsed >= Duration::from_secs_f64(off_secs) {
                if self.screen_power_state != ScreenPowerState::Off {
                    self.screen_power_state = ScreenPowerState::Off;
                    self.cursor_visible = false;
                    tracing::debug!("Display off after {off_secs}s idle");
                }
                return;
            }
        }

        // Check screen-blank timeout.
        if let Some(blank_secs) = self.config.screen_blank_timeout_secs {
            if elapsed >= Duration::from_secs_f64(blank_secs) {
                if self.screen_power_state == ScreenPowerState::Active {
                    self.screen_power_state = ScreenPowerState::Blanked;
                    self.cursor_visible = false;
                    tracing::debug!("Screen blanked after {blank_secs}s idle");
                }
                return;
            }
        }
    }

    // --- Window params ---

    /// Compute effective decoration parameters for a window, applying any
    /// matching window rules over the global config defaults.
    pub fn effective_window_params_for(&self, mw: &ManagedWindow) -> EffectiveWindowParams {
        self.effective_window_params(mw.title().as_deref(), mw.app_id().as_deref(), mw.fullscreen)
    }

    pub fn effective_window_params(&self, title: Option<&str>, app_id: Option<&str>, is_fullscreen: bool) -> EffectiveWindowParams {
        let mut params = EffectiveWindowParams {
            padding: self.config.padding,
            corner_radius: self.config.corner_radius,
            inner_padding: self.config.inner_padding,
            inner_padding_color: self.config.inner_padding_color,
            border_thickness: self.config.border_thickness,
            border_color: self.config.border_color,
            fullscreen: false,
            idle_hold: IdleHoldPolicy::Default,
        };
        for cr in &self.compiled_rules {
            let title_matches = cr.title_re.as_ref().map_or(true, |re| {
                title.map_or(false, |t| re.is_match(t))
            });
            let app_id_matches = cr.app_id_re.as_ref().map_or(true, |re| {
                app_id.map_or(false, |a| re.is_match(a))
            });
            if title_matches && app_id_matches {
                if let Some(v) = cr.rule.padding { params.padding = v; }
                if let Some(v) = cr.rule.corner_radius { params.corner_radius = v; }
                if let Some(v) = cr.rule.inner_padding { params.inner_padding = v; }
                if let Some(v) = cr.rule.inner_padding_color { params.inner_padding_color = v; }
                if let Some(v) = cr.rule.border_thickness { params.border_thickness = v; }
                if let Some(v) = cr.rule.border_color { params.border_color = v; }
                if let Some(v) = cr.rule.fullscreen { params.fullscreen = v; }
                if let Some(ref v) = cr.rule.idle_hold { params.idle_hold = v.clone(); }
            }
        }
        if is_fullscreen || params.fullscreen {
            params.fullscreen = true;
            params.padding = 0;
            params.corner_radius = 0.0;
            params.inner_padding = 0;
            params.border_thickness = 0;
        }
        params
    }

    /// The total decoration box for a window: layer zone minus outer padding.
    /// When fullscreen, uses the full output size.
    fn window_outer_area_for(&self, params: &EffectiveWindowParams) -> Rectangle<i32, Logical> {
        if params.fullscreen {
            Rectangle::new(Point::from((0, 0)), self.output_size)
        } else {
            let layer_map = layer_map_for_output(&self.output);
            let zone = layer_map.non_exclusive_zone();
            let p = params.padding;
            Rectangle::new(
                Point::from((zone.loc.x + p, zone.loc.y + p)),
                Size::from(((zone.size.w - 2 * p).max(1), (zone.size.h - 2 * p).max(1))),
            )
        }
    }

    /// The content area for a window after subtracting border and inner padding.
    pub fn window_content_area_for(&self, params: &EffectiveWindowParams) -> Rectangle<i32, Logical> {
        let outer = self.window_outer_area_for(params);
        let inset = params.border_thickness + params.inner_padding;
        Rectangle::new(
            Point::from((outer.loc.x + inset, outer.loc.y + inset)),
            Size::from(((outer.size.w - 2 * inset).max(1), (outer.size.h - 2 * inset).max(1))),
        )
    }

    /// Returns the screen area available for window content using global config
    /// defaults (no per-window rules). Used for pointer hit-testing and
    /// handlers that don't have a specific window context yet.
    pub fn window_area(&self) -> Rectangle<i32, Logical> {
        let layer_map = layer_map_for_output(&self.output);
        let zone = layer_map.non_exclusive_zone();
        let p = self.config.padding;
        let inset = self.config.border_thickness + self.config.inner_padding;
        let total = p + inset;
        Rectangle::new(
            Point::from((zone.loc.x + total, zone.loc.y + total)),
            Size::from(((zone.size.w - 2 * total).max(1), (zone.size.h - 2 * total).max(1))),
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
        let params = self.windows.iter()
            .find(|w| w.id == id)
            .map(|mw| self.effective_window_params_for(mw))
            .unwrap_or_else(|| self.effective_window_params(None, None, false));
        let content_area = self.window_content_area_for(&params);
        if let Some(mw) = self.windows.iter().find(|w| w.id == id) {
            if let Some(toplevel) = mw.window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some(content_area.size);
                    state.states.set(xdg_toplevel::State::Activated);
                    if params.fullscreen {
                        state.states.set(xdg_toplevel::State::Fullscreen);
                    } else {
                        state.states.unset(xdg_toplevel::State::Fullscreen);
                    }
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
        let first = self.windows.iter().find(|w| w.desktop == desktop && w.window.alive()).map(|w| w.id);
        let prev = self.current_window_id;
        self.current_window_id = first;
        if let Some(prev_id) = prev {
            if let Some(mw) = self.windows.iter().find(|w| w.id == prev_id) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| {
                        s.states.unset(xdg_toplevel::State::Activated);
                    });
                    t.send_pending_configure();
                }
            }
        }
        if let Some(new_id) = first {
            let params = self.windows.iter()
                .find(|w| w.id == new_id)
                .map(|mw| self.effective_window_params_for(mw))
                .unwrap_or_else(|| self.effective_window_params(None, None, false));
            let content_area = self.window_content_area_for(&params);
            if let Some(mw) = self.windows.iter().find(|w| w.id == new_id) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = Some(content_area.size);
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

    pub fn toggle_fullscreen(&mut self, id: Option<u64>) {
        let target = id.or(self.current_window_id);
        if let Some(wid) = target {
            if let Some(mw) = self.windows.iter_mut().find(|w| w.id == wid) {
                mw.fullscreen = !mw.fullscreen;
            }
            // Reconfigure the window with updated sizing.
            let params = self.windows.iter()
                .find(|w| w.id == wid)
                .map(|mw| self.effective_window_params_for(mw))
                .unwrap_or_else(|| self.effective_window_params(None, None, false));
            let content_area = self.window_content_area_for(&params);
            if let Some(mw) = self.windows.iter().find(|w| w.id == wid) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = Some(content_area.size);
                        if params.fullscreen {
                            s.states.set(xdg_toplevel::State::Fullscreen);
                        } else {
                            s.states.unset(xdg_toplevel::State::Fullscreen);
                        }
                    });
                    t.send_pending_configure();
                }
            }
        }
    }

    pub fn process_pending(&mut self) {
        let dead: Vec<u64> = self.windows.iter().filter(|w| !w.window.alive()).map(|w| w.id).collect();
        for id in dead {
            self.remove_window(id);
        }
        self.layer_surfaces.retain(|s| s.alive());
        // Clean up idle inhibitors for dead surfaces.
        self.idle_inhibit_surfaces.retain(|s| s.alive());
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            match cmd {
                IpcCommand::ShowDesktop(n) => {
                    self.show_desktop(n);
                    self.record_activity();
                },
                IpcCommand::ShowWindow(id) => {
                    self.show_window(id);
                    self.record_activity();
                },
                IpcCommand::KillWindow(id) => {
                    self.kill_window(id);
                    self.record_activity();
                },
                IpcCommand::ToggleFullscreen(id) => {
                    self.toggle_fullscreen(id);
                    self.record_activity();
                },
            }
        }
    }

    fn remove_window(&mut self, id: u64) {
        if self.current_window_id == Some(id) {
            let next =
                self
                    .windows
                    .iter()
                    .filter(|w| w.id != id && w.desktop == self.current_desktop && w.window.alive())
                    .map(|w| w.id)
                    .next();
            self.current_window_id = next;
            if let Some(next_id) = next {
                let params = self.windows.iter()
                    .find(|w| w.id == next_id)
                    .map(|mw| self.effective_window_params_for(mw))
                    .unwrap_or_else(|| self.effective_window_params(None, None, false));
                let content_area = self.window_content_area_for(&params);
                if let Some(mw) = self.windows.iter().find(|w| w.id == next_id) {
                    if let Some(t) = mw.window.toplevel() {
                        t.with_pending_state(|s| {
                            s.size = Some(content_area.size);
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
        let shared = self.ipc_shared.lock().unwrap();

        // Ignore error — no receivers connected is fine.
        let _ = shared.event_tx.send(event);
    }

    pub fn sync_ipc_windows(&self) {
        let windows: Vec<WindowInfo> =
            self
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

        // Layer shell: Overlay and Top (in front of windows) — pushed first = highest z-order
        {
            let layer_map = layer_map_for_output(&self.output);
            for layer in [Layer::Overlay, Layer::Top] {
                for surface in layer_map.layers_on(layer) {
                    let loc =
                        layer_map
                            .layer_geometry(surface)
                            .map(|g| Point::<i32, Physical>::from((g.loc.x, g.loc.y)))
                            .unwrap_or_default();
                    for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                        renderer,
                        surface.wl_surface(),
                        loc,
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    ) {
                        elements.push(elem);
                    }
                }
            }
        }

        // Current window: popups + surface + decorations (front to back)
        if let Some(id) = self.current_window_id {
            if let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) {
                let params = self.effective_window_params_for(mw);
                let outer_rect = self.window_outer_area_for(&params);
                let content_area = self.window_content_area_for(&params);
                let radius = params.corner_radius;
                let shader = self.rounded_rect_shader.as_ref();

                if let Some(toplevel) = mw.window.toplevel() {
                    // Center the window if its committed geometry is valid and
                    // smaller than the content area (e.g. non-resizable windows).
                    let geo = mw.window.geometry();
                    let (origin_x, origin_y) = if geo.size.w > 0 && geo.size.h > 0
                        && (geo.size.w < content_area.size.w || geo.size.h < content_area.size.h)
                    {
                        let cx = content_area.loc.x + (content_area.size.w - geo.size.w) / 2;
                        let cy = content_area.loc.y + (content_area.size.h - geo.size.h) / 2;
                        (cx - geo.loc.x, cy - geo.loc.y)
                    } else {
                        (content_area.loc.x, content_area.loc.y)
                    };

                    // Popups (in front of window surface)
                    for (popup, popup_offset) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                        let popup_geo = popup.geometry();
                        let popup_loc = Point::<i32, Physical>::from((
                            origin_x + geo.loc.x + popup_offset.x - popup_geo.loc.x,
                            origin_y + geo.loc.y + popup_offset.y - popup_geo.loc.y,
                        ));
                        for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                            renderer,
                            popup.wl_surface(),
                            popup_loc,
                            1.0,
                            1.0,
                            Kind::Unspecified,
                        ) {
                            elements.push(elem);
                        }
                    }

                    // Window surface
                    let win_loc = Point::<i32, Physical>::from((origin_x, origin_y));
                    for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                        renderer,
                        toplevel.wl_surface(),
                        win_loc,
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    ) {
                        elements.push(elem);
                    }
                }

                // Inner padding (behind window surface)
                if params.inner_padding > 0 {
                    let ip_rect = shrink_rect(outer_rect, params.border_thickness);
                    let ip_radius = (radius - params.border_thickness as f32).max(0.0);
                    push_colored_rect(
                        &mut elements,
                        ip_rect,
                        params.inner_padding_color,
                        ip_radius,
                        shader,
                    );
                }

                // Border (behind inner padding, fills outer_rect)
                if params.border_thickness > 0 {
                    push_colored_rect(
                        &mut elements,
                        outer_rect,
                        params.border_color,
                        radius,
                        shader,
                    );
                }
            }
        }

        // Layer shell: Bottom and Background (behind windows)
        {
            let layer_map = layer_map_for_output(&self.output);
            for layer in [Layer::Bottom, Layer::Background] {
                for surface in layer_map.layers_on(layer) {
                    let loc =
                        layer_map
                            .layer_geometry(surface)
                            .map(|g| Point::<i32, Physical>::from((g.loc.x, g.loc.y)))
                            .unwrap_or_default();
                    for elem in render_elements_from_surface_tree::<GlesRenderer, CompElement>(
                        renderer,
                        surface.wl_surface(),
                        loc,
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    ) {
                        elements.push(elem);
                    }
                }
            }
        }

        // Background image (pushed last = lowest z-order, behind everything)
        if let Some((bg, (img_w, img_h))) = &self.background_buffer {
            let screen_w = self.output_size.w as f64;
            let screen_h = self.output_size.h as f64;
            let img_w = *img_w as f64;
            let img_h = *img_h as f64;
            let [align_x, align_y] = self.config.background_align;

            let src_rect = match self.config.background_size {
                BackgroundSize::Cover => cover_src_rect(img_w, img_h, screen_w, screen_h, align_x, align_y),
                BackgroundSize::MinCover => {
                    if img_w >= screen_w && img_h >= screen_h {
                        let src_x = (img_w - screen_w) * align_x;
                        let src_y = (img_h - screen_h) * align_y;
                        Rectangle::<f64, Logical>::new(
                            Point::from((src_x, src_y)),
                            Size::from((screen_w, screen_h)),
                        )
                    } else {
                        cover_src_rect(img_w, img_h, screen_w, screen_h, align_x, align_y)
                    }
                },
            };
            elements.push(
                TextureRenderElement::from_texture_buffer(
                    Point::from((0.0f64, 0.0f64)),
                    bg,
                    None,
                    Some(src_rect),
                    Some(self.output_size),
                    Kind::Unspecified,
                ).into(),
            );
        }

        elements
    }

    /// Compute the surface origin for the current window, centering its
    /// geometry within the content area when it is smaller (e.g. non-resizable).
    pub fn current_window_surface_origin(&self) -> Option<Point<i32, Logical>> {
        let id = self.current_window_id?;
        let mw = self.windows.iter().find(|w| w.id == id && w.window.alive())?;
        let params = self.effective_window_params_for(mw);
        let content_area = self.window_content_area_for(&params);
        let geo = mw.window.geometry();
        if geo.size.w > 0 && geo.size.h > 0
            && (geo.size.w < content_area.size.w || geo.size.h < content_area.size.h)
        {
            let cx = content_area.loc.x + (content_area.size.w - geo.size.w) / 2;
            let cy = content_area.loc.y + (content_area.size.h - geo.size.h) / 2;
            Some(Point::from((cx - geo.loc.x, cy - geo.loc.y)))
        } else {
            Some(Point::from((content_area.loc.x, content_area.loc.y)))
        }
    }

    pub fn send_frames(&self, time_ms: u32) {
        use smithay::wayland::compositor::{
            SurfaceAttributes,
            TraversalAction,
            with_surface_tree_downward,
        };

        let send_to = |surface: &WlSurface| {
            with_surface_tree_downward(surface, (), |_, _, &()| TraversalAction::DoChildren(()), |_, states, &()| {
                for cb in states.cached_state.get::<SurfaceAttributes>().current().frame_callbacks.drain(..) {
                    cb.done(time_ms);
                }
            }, |_, _, &()| true);
        };
        if let Some(id) = self.current_window_id {
            if let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) {
                if let Some(t) = mw.window.toplevel() {
                    send_to(t.wl_surface());
                    for (popup, _) in PopupManager::popups_for_surface(t.wl_surface()) {
                        send_to(popup.wl_surface());
                    }
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

// ---------------------------------------------------------------------------
// Helper: background src rect for cover mode
// ---------------------------------------------------------------------------

fn cover_src_rect(
    img_w: f64,
    img_h: f64,
    screen_w: f64,
    screen_h: f64,
    align_x: f64,
    align_y: f64,
) -> Rectangle<f64, Logical> {
    let scale = (screen_w / img_w).max(screen_h / img_h);
    let src_w = screen_w / scale;
    let src_h = screen_h / scale;
    let src_x = (img_w - src_w) * align_x;
    let src_y = (img_h - src_h) * align_y;
    Rectangle::new(Point::from((src_x, src_y)), Size::from((src_w, src_h)))
}

// ---------------------------------------------------------------------------
// Helper: shrink a logical rect by `amount` on all sides
// ---------------------------------------------------------------------------

fn shrink_rect(rect: Rectangle<i32, Logical>, amount: i32) -> Rectangle<i32, Logical> {
    Rectangle::new(
        Point::from((rect.loc.x + amount, rect.loc.y + amount)),
        Size::from(((rect.size.w - 2 * amount).max(1), (rect.size.h - 2 * amount).max(1))),
    )
}

// ---------------------------------------------------------------------------
// Helper: push a solid-colored (optionally rounded) rectangle element
// ---------------------------------------------------------------------------

fn push_colored_rect(
    elements: &mut Vec<CompElement>,
    rect: Rectangle<i32, Logical>,
    color: [f32; 4],
    corner_radius: f32,
    shader: Option<&GlesPixelProgram>,
) {
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return;
    }
    if corner_radius > 0.0 {
        if let Some(prog) = shader {
            let elem = PixelShaderElement::new(
                prog.clone(),
                rect,
                None,
                1.0,
                vec![
                    Uniform::new("u_color", UniformValue::_4f(color[0], color[1], color[2], color[3])),
                    Uniform::new("u_radius", UniformValue::_1f(corner_radius)),
                ],
                Kind::Unspecified,
            );
            elements.push(CompElement::PixelShader(elem));
            return;
        }
    }
    // No rounding (radius=0 or shader unavailable) — solid rectangle.
    let buf = SolidColorBuffer::new(rect.size, Color32F::new(color[0], color[1], color[2], color[3]));
    let elem = SolidColorRenderElement::from_buffer(
        &buf,
        Point::<i32, Physical>::from((rect.loc.x, rect.loc.y)),
        1.0f64,
        1.0,
        Kind::Unspecified,
    );
    elements.push(CompElement::Solid(elem));
}

// ---------------------------------------------------------------------------
// Helper: compile window rules into regex-bearing structs
// ---------------------------------------------------------------------------

fn compile_rules(rules: &[WindowRule]) -> Vec<CompiledRule> {
    rules
        .iter()
        .map(|rule| {
            let title_re = rule.title.as_deref().and_then(|pat| {
                Regex::new(pat)
                    .map_err(|e| tracing::warn!("Invalid title regex {:?}: {e}", pat))
                    .ok()
            });
            let app_id_re = rule.app_id.as_deref().and_then(|pat| {
                Regex::new(pat)
                    .map_err(|e| tracing::warn!("Invalid app_id regex {:?}: {e}", pat))
                    .ok()
            });
            CompiledRule { title_re, app_id_re, rule: rule.clone() }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fragment shader for rounded-rect decoration elements.
// Smithay prepends "#version 100\n" automatically.
// Provided uniforms: v_coords (varying, 0..1), size (vec2, auto), alpha (float, auto).
// ---------------------------------------------------------------------------

pub const ROUNDED_RECT_SHADER: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform vec4 u_color;
uniform float u_radius;

float roundedBoxSDF(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec2 pixel_pos = v_coords * size;
    vec2 half_size = size * 0.5;
    float r = min(u_radius, min(half_size.x, half_size.y));
    float d = roundedBoxSDF(pixel_pos - half_size, half_size, r);
    float shape_alpha = 1.0 - smoothstep(-0.5, 0.5, d);
    float a = u_color.a * alpha * shape_alpha;
    gl_FragColor = vec4(u_color.rgb * a, a);
}
"#;
