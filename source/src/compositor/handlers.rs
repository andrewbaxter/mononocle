use {
    crate::{
        compositor::state::{
            ClientState,
            ManagedWindow,
            State,
            next_window_id,
        },
        ipc::WindowEvent,
    },
    smithay::{
        delegate_compositor,
        delegate_data_device,
        delegate_layer_shell,
        delegate_output,
        delegate_seat,
        delegate_shm,
        delegate_xdg_shell,
        desktop::{
            LayerSurface as DesktopLayerSurface,
            PopupKind,
            Window,
            layer_map_for_output,
        },
        input::{
            Seat,
            SeatHandler,
            SeatState,
            pointer::CursorImageStatus,
        },
        reexports::{
            wayland_protocols::xdg::shell::server::xdg_toplevel,
            wayland_server::protocol::{
                wl_buffer::WlBuffer,
                wl_output::WlOutput,
                wl_seat,
                wl_surface::WlSurface,
            },
        },
        utils::{
            IsAlive,
            Point,
            Rectangle,
            SERIAL_COUNTER,
            Serial,
        },
        wayland::{
            buffer::BufferHandler,
            compositor::{
                CompositorClientState,
                CompositorHandler,
                CompositorState,
                with_states,
            },
            output::OutputHandler,
            selection::{
                SelectionHandler,
                data_device::{
                    ClientDndGrabHandler,
                    DataDeviceHandler,
                    DataDeviceState,
                    ServerDndGrabHandler,
                },
            },
            shell::{
                wlr_layer::{
                    Layer,
                    LayerSurface as WlrLayerSurface,
                    WlrLayerShellHandler,
                    WlrLayerShellState,
                },
                xdg::{
                    PopupSurface,
                    PositionerState,
                    ToplevelSurface,
                    XdgShellHandler,
                    XdgShellState,
                    XdgToplevelSurfaceData,
                },
            },
            shm::{
                ShmHandler,
                ShmState,
            },
        },
    },
    std::os::unix::io::OwnedFd,
};

// --- CompositorHandler ---
impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<
        'a,
    >(&self, client: &'a smithay::reexports::wayland_server::Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.popup_manager.commit(surface);
        {
            let mut layer_map = layer_map_for_output(&self.output);
            layer_map.cleanup();
        }
        self.sync_ipc_windows();
    }
}

delegate_compositor!(State);

// --- BufferHandler ---
impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) { }
}

// --- ShmHandler ---
impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(State);

// --- XdgShellHandler ---
impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = next_window_id();
        let window = Window::new_wayland_window(surface.clone());
        let desktop = self.current_desktop;
        // Try to read title/app_id already sent by the client before the
        // initial commit, so window rules are applied correctly from the start.
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|g| g.title.clone())
        });
        let app_id = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|g| g.app_id.clone())
        });
        let params = self.effective_window_params(title.as_deref(), app_id.as_deref());
        let content_area = self.window_content_area_for(&params);
        surface.with_pending_state(|state| {
            state.size = Some(content_area.size);
            state.states.set(xdg_toplevel::State::Maximized);
        });
        surface.send_configure();
        let is_first_visible =
            self.current_window_id.is_none() ||
                !self.windows.iter().any(|w| w.desktop == desktop && w.window.alive());
        let managed = ManagedWindow {
            id,
            window,
            desktop,
        };
        let info = managed.to_info(if is_first_visible {
            Some(id)
        } else {
            None
        });
        self.windows.push(managed);
        if is_first_visible {
            self.current_window_id = Some(id);
            if let Some(mw) = self.windows.iter().find(|w| w.id == id) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|state| {
                        state.size = Some(content_area.size);
                        state.states.set(xdg_toplevel::State::Activated);
                    });
                    t.send_pending_configure();
                }
            }
            if let Some(mw) = self.windows.iter().find(|w| w.id == id) {
                if let Some(t) = mw.window.toplevel() {
                    let serial = SERIAL_COUNTER.next_serial();
                    if let Some(kb) = self.seat.get_keyboard() {
                        kb.set_focus(self, Some(t.wl_surface().clone()), serial);
                    }
                }
            }
        }
        self.push_event(WindowEvent::WindowCreated { window: info });
        self.sync_ipc_windows();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        let area = self.window_area();
        let requested_size = surface.with_pending_state(|state| state.positioner.rect_size);

        // Center popup within the window area; position is relative to parent (at
        // area.loc)
        let rel_x = ((area.size.w - requested_size.w) / 2).max(0);
        let rel_y = ((area.size.h - requested_size.h) / 2).max(0);
        surface.with_pending_state(|state| {
            state.geometry = Rectangle::new(Point::from((rel_x, rel_y)), requested_size);
        });
        surface.send_configure().ok();
        self.popup_manager.track_popup(PopupKind::Xdg(surface)).ok();
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) { }

    fn reposition_request(&mut self, surface: PopupSurface, _positioner: PositionerState, token: u32) {
        surface.send_repositioned(token);
    }
}

delegate_xdg_shell!(State);

// --- WlrLayerShellHandler ---
impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let desktop_surface = DesktopLayerSurface::new(surface, namespace);
        {
            let mut layer_map = layer_map_for_output(&self.output);
            layer_map.map_layer(&desktop_surface).ok();
        }
        desktop_surface.layer_surface().send_configure();
        self.layer_surfaces.push(desktop_surface);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.layer_surfaces.retain(|s| s.wl_surface() != surface.wl_surface());
        let mut layer_map = layer_map_for_output(&self.output);
        layer_map.cleanup();
    }
}

delegate_layer_shell!(State);

// --- SeatHandler ---
impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) { }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) { }
}

delegate_seat!(State);

// --- DataDevice ---
impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State { }

impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) { }
}

delegate_data_device!(State);

// --- OutputHandler ---
impl OutputHandler for State { }

delegate_output!(State);
