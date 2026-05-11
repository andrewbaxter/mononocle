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
        delegate_idle_inhibit,
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
        // Update the bounding box for any window whose surface was committed.
        for mw in &self.windows {
            if mw.window.toplevel().map_or(false, |t| t.wl_surface() == surface) {
                mw.window.on_commit();
            }
        }
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
        // Notify the client that its surface is on our output so it knows the
        // correct scale/transform before it renders its first frame.
        self.output.enter(surface.wl_surface());

        let is_first_visible =
            self.current_window_id.is_none() ||
                !self.windows.iter().any(|w| w.desktop == desktop && w.window.alive());
        let managed = ManagedWindow {
            id,
            window,
            desktop,
            fullscreen: is_fullscreen,
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

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // Use the positioner's computed geometry which respects anchor rect,
        // gravity, and offset as requested by the client (e.g. menu placement).
        let geo = positioner.get_geometry();
        surface.with_pending_state(|state| {
            state.geometry = geo;
        });
        surface.send_configure().ok();
        self.popup_manager.track_popup(PopupKind::Xdg(surface)).ok();
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        use smithay::desktop::{PopupKeyboardGrab, find_popup_root_surface};

        let popup = PopupKind::Xdg(surface);
        match find_popup_root_surface(&popup) {
            Ok(root) => {
                match self.popup_manager.grab_popup::<State>(root, popup, &self.seat, serial) {
                    Ok(grab) => {
                        if let Some(kb) = self.seat.get_keyboard() {
                            let kb_grab = PopupKeyboardGrab::new(&grab);
                            kb.set_grab(self, kb_grab, serial);
                        }
                        if let Some(ptr) = self.seat.get_pointer() {
                            let ptr_grab = PopupPointerGrabSkipFirstRelease::new(&grab);
                            ptr.set_grab(self, ptr_grab, serial, smithay::input::pointer::Focus::Keep);
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Popup grab failed: {err}");
                    }
                }
            }
            Err(err) => {
                tracing::warn!("find_popup_root_surface failed: {err}");
            }
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, _positioner: PositionerState, token: u32) {
        surface.send_repositioned(token);
    }
}

/// Wraps [`PopupPointerGrab`] but skips forwarding the first button release
/// to the client.  This works around toolkits (GTK) that dismiss the popup
/// when they see the release of the button that opened the menu.
struct PopupPointerGrabSkipFirstRelease {
    inner: smithay::desktop::PopupPointerGrab<State>,
    skip_next_release: bool,
}

impl PopupPointerGrabSkipFirstRelease {
    fn new(grab: &smithay::desktop::PopupGrab<State>) -> Self {
        Self {
            inner: smithay::desktop::PopupPointerGrab::new(grab),
            skip_next_release: true,
        }
    }
}

impl smithay::input::pointer::PointerGrab<State> for PopupPointerGrabSkipFirstRelease {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, smithay::utils::Point<f64, smithay::utils::Logical>)>,
        event: &smithay::input::pointer::MotionEvent,
    ) {
        self.inner.motion(data, handle, focus, event);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, smithay::utils::Point<f64, smithay::utils::Logical>)>,
        event: &smithay::input::pointer::RelativeMotionEvent,
    ) {
        self.inner.relative_motion(data, handle, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::ButtonEvent,
    ) {
        if self.skip_next_release && event.state == smithay::backend::input::ButtonState::Released {
            self.skip_next_release = false;
            return;
        }
        self.inner.button(data, handle, event);
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        details: smithay::input::pointer::AxisFrame,
    ) {
        self.inner.axis(data, handle, details);
    }

    fn frame(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
    ) {
        self.inner.frame(data, handle);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureSwipeBeginEvent,
    ) {
        self.inner.gesture_swipe_begin(data, handle, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureSwipeUpdateEvent,
    ) {
        self.inner.gesture_swipe_update(data, handle, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureSwipeEndEvent,
    ) {
        self.inner.gesture_swipe_end(data, handle, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchBeginEvent,
    ) {
        self.inner.gesture_pinch_begin(data, handle, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchUpdateEvent,
    ) {
        self.inner.gesture_pinch_update(data, handle, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GesturePinchEndEvent,
    ) {
        self.inner.gesture_pinch_end(data, handle, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureHoldBeginEvent,
    ) {
        self.inner.gesture_hold_begin(data, handle, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut smithay::input::pointer::PointerInnerHandle<'_, State>,
        event: &smithay::input::pointer::GestureHoldEndEvent,
    ) {
        self.inner.gesture_hold_end(data, handle, event);
    }

    fn start_data(&self) -> &smithay::input::pointer::GrabStartData<State> {
        self.inner.start_data()
    }

    fn unset(&mut self, data: &mut State) {
        self.inner.unset(data);
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

// --- IdleInhibitHandler ---
impl smithay::wayland::idle_inhibit::IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit_surfaces.insert(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit_surfaces.remove(&surface);
    }
}

delegate_idle_inhibit!(State);
