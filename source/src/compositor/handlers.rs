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
        backend::{
            input::ButtonState,
            renderer::utils::on_commit_buffer_handler,
        },
        delegate_compositor,
        delegate_data_device,
        delegate_idle_inhibit,
        delegate_layer_shell,
        delegate_output,
        delegate_seat,
        delegate_shm,
        delegate_xdg_shell,
        desktop::{
            LayerSurface as DesktopLayerSurface,
            PopupGrab,
            PopupKeyboardGrab,
            PopupKind,
            PopupPointerGrab,
            Window,
            find_popup_root_surface,
            layer_map_for_output,
        },
        input::{
            Seat,
            SeatHandler,
            SeatState,
            pointer::{
                AxisFrame,
                ButtonEvent,
                CursorImageStatus,
                Focus,
                GestureHoldBeginEvent,
                GestureHoldEndEvent,
                GesturePinchBeginEvent,
                GesturePinchEndEvent,
                GesturePinchUpdateEvent,
                GestureSwipeBeginEvent,
                GestureSwipeEndEvent,
                GestureSwipeUpdateEvent,
                GrabStartData,
                MotionEvent,
                PointerGrab,
                PointerInnerHandle,
                RelativeMotionEvent,
            },
        },
        reexports::{
            wayland_protocols::xdg::shell::server::xdg_toplevel,
            wayland_server::{
                Client,
                protocol::{
                    wl_buffer::WlBuffer,
                    wl_output::WlOutput,
                    wl_seat,
                    wl_surface::WlSurface,
                },
            },
        },
        utils::{
            IsAlive,
            Logical,
            Point,
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
            idle_inhibit::IdleInhibitHandler,
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

delegate_compositor!(State);

delegate_data_device!(State);

delegate_idle_inhibit!(State);

delegate_layer_shell!(State);

delegate_output!(State);

delegate_seat!(State);

delegate_shm!(State);

delegate_xdg_shell!(State);

struct PopupPointerGrabSkipFirstRelease {
    inner: PopupPointerGrab<State>,
    skip_next_release: bool,
}

impl PopupPointerGrabSkipFirstRelease {
    fn new(grab: &PopupGrab<State>) -> Self {
        Self {
            inner: PopupPointerGrab::new(grab),
            skip_next_release: true,
        }
    }
}

impl PointerGrab<State> for PopupPointerGrabSkipFirstRelease {
    fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
        self.inner.axis(data, handle, details);
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        if self.skip_next_release && event.state == ButtonState::Released {
            self.skip_next_release = false;
            return;
        }
        self.inner.button(data, handle, event);
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        self.inner.frame(data, handle);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        self.inner.gesture_hold_begin(data, handle, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        self.inner.gesture_hold_end(data, handle, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        self.inner.gesture_pinch_begin(data, handle, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        self.inner.gesture_pinch_end(data, handle, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        self.inner.gesture_pinch_update(data, handle, event);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        self.inner.gesture_swipe_begin(data, handle, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        self.inner.gesture_swipe_end(data, handle, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.inner.gesture_swipe_update(data, handle, event);
    }

    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        self.inner.motion(data, handle, focus, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        self.inner.relative_motion(data, handle, focus, event);
    }

    fn start_data(&self) -> &GrabStartData<State> {
        self.inner.start_data()
    }

    fn unset(&mut self, data: &mut State) {
        self.inner.unset(data);
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) { }
}

impl ClientDndGrabHandler for State { }

impl CompositorHandler for State {
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.popup_manager.commit(surface);
        let mut reapply_id = None;
        for mw in &self.windows {
            if mw.window.toplevel().map_or(false, |t| t.wl_surface() == surface) {
                mw.window.on_commit();
                reapply_id = Some(mw.id);
            }
        }
        if let Some(wid) = reapply_id {
            self.reapply_window_rules(wid);
        }
        {
            let mut layer_map = layer_map_for_output(&self.output);
            layer_map.cleanup();
        }
        self.sync_ipc_windows();
    }

    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit_surfaces.insert(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit_surfaces.remove(&surface);
    }
}

impl OutputHandler for State { }

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) { }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) { }

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) { }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl WlrLayerShellHandler for State {
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.layer_surfaces.retain(|s| s.wl_surface() != surface.wl_surface());
        let mut layer_map = layer_map_for_output(&self.output);
        layer_map.cleanup();
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

    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }
}

impl XdgShellHandler for State {
    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        let popup = PopupKind::Xdg(surface);
        match find_popup_root_surface(&popup) {
            Ok(root) => {
                match self.popup_manager.grab_popup::<State>(root, popup, &self.seat, serial) {
                    Ok(grab) => {
                        if let Some(kb) = self.seat.get_keyboard() {
                            kb.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                        }
                        if let Some(ptr) = self.seat.get_pointer() {
                            ptr.set_grab(self, PopupPointerGrabSkipFirstRelease::new(&grab), serial, Focus::Keep);
                        }
                    },
                    Err(err) => {
                        tracing::warn!("Popup grab failed: {err}");
                    },
                }
            },
            Err(err) => {
                tracing::warn!("find_popup_root_surface failed: {err}");
            },
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        surface.send_configure().ok();
        self.popup_manager.track_popup(PopupKind::Xdg(surface)).ok();
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = next_window_id();
        let window = Window::new_wayland_window(surface.clone());
        let client_pid = self.pid_for_surface(surface.wl_surface());
        let desktop = client_pid.and_then(|pid| self.desktop_for_pid(pid)).unwrap_or(self.current_desktop);
        self.ensure_desktop(desktop);
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
        let params = self.effective_window_params(title.as_deref(), app_id.as_deref(), false);
        let is_fullscreen = params.fullscreen;
        let content_area = self.window_content_area_for(&params);
        surface.with_pending_state(|state| {
            state.size = Some(content_area.size);
            state.states.set(xdg_toplevel::State::Maximized);
            state.states.set(xdg_toplevel::State::TiledLeft);
            state.states.set(xdg_toplevel::State::TiledRight);
            state.states.set(xdg_toplevel::State::TiledTop);
            state.states.set(xdg_toplevel::State::TiledBottom);
            if is_fullscreen {
                state.states.set(xdg_toplevel::State::Fullscreen);
            }
        });
        surface.send_configure();
        self.output.enter(surface.wl_surface());
        let is_first_visible =
            self.current_window_id.is_none() ||
                !self.windows.iter().any(|w| w.desktop == desktop && w.window.alive());
        let managed = ManagedWindow {
            id: id,
            window: window,
            desktop: desktop,
            fullscreen: is_fullscreen,
        };
        let info = managed.to_info(if is_first_visible {
            Some(id)
        } else {
            None
        });
        self.windows.push(managed);
        if let Some(pid) = client_pid {
            self.associate_pid_tree_with_desktop(pid, desktop);
        }
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

    fn reposition_request(&mut self, surface: PopupSurface, _positioner: PositionerState, token: u32) {
        surface.send_repositioned(token);
    }

    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
}
