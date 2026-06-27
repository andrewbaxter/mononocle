use {
    aargvark::{
        Aargvark,
        vark,
    },
    image::ImageReader,
    mononocle::{
        compositor::{
            config::{
                BackgroundSpec,
                Config,
            },
            ipc_server::{
                SharedIpcState,
                spawn_ipc_server,
            },
            state::{
                ClientState,
                LOCK_SHADER,
                LockInputState,
                ROUNDED_RECT_SHADER,
                ScreenPowerState,
                State,
            },
        },
        ipc::{
            CheckPassword,
            unlock_protocol,
        },
    },
    smithay::{
        backend::{
            allocator::Fourcc,
            input::{
                AbsolutePositionEvent,
                Event as InputEventTrait,
                InputEvent,
                KeyState,
                KeyboardKeyEvent,
                PointerButtonEvent,
            },
            renderer::{
                Color32F,
                Frame,
                Renderer,
                element::texture::TextureBuffer,
                gles::{
                    GlesRenderer,
                    GlesTexture,
                    UniformName,
                    UniformType,
                },
                utils::draw_render_elements,
            },
            winit::{
                WinitEvent,
                WinitInput,
                self,
            },
        },
        desktop::{
            WindowSurfaceType,
            layer_map_for_output,
        },
        input::{
            keyboard::{
                FilterResult,
                Keysym,
            },
            pointer::{
                ButtonEvent,
                MotionEvent,
            },
        },
        output::Mode,
        reexports::{
            wayland_server::{
                Display,
                ListeningSocket,
            },
            winit::platform::pump_events::PumpStatus,
        },
        utils::{
            Buffer,
            IsAlive,
            Logical,
            Physical,
            Point,
            Rectangle,
            SERIAL_COUNTER,
            Size,
            Transform,
        },
    },
    std::{
        error::Error,
        fs::read_to_string,
        path::PathBuf,
        process::exit,
        sync::{
            Arc,
            Mutex,
            mpsc::channel,
        },
        thread::spawn,
        time::{
            Duration,
            Instant,
        },
    },
    tokio::runtime::Builder as RuntimeBuilder,
};

#[derive(Aargvark)]
struct Args {
    config: Option<PathBuf>,
    default_config: Option<()>,
    validate: Option<()>,
}

fn collect_background_specs(config: &Config) -> Vec<&BackgroundSpec> {
    let mut specs: Vec<&BackgroundSpec> = Vec::new();
    if let Some(spec) = &config.default_style.background {
        specs.push(spec);
    }
    for spec in config.desktop_backgrounds.values() {
        specs.push(spec);
    }
    for rule in &config.window_rules {
        if let Some(spec) = &rule.style.background {
            specs.push(spec);
        }
    }
    specs
}

fn main() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
    let args: Args = vark();
    if args.default_config.is_some() {
        let json = serde_json::to_string_pretty(&Config::default()).expect("serialize default config");
        println!("{json}");
        return;
    }
    let config: Config = if let Some(path) = args.config {
        serde_json::from_str(
            &read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read config {}: {e}", path.display())),
        ).unwrap_or_else(|e| panic!("Failed to parse config {}: {e}", path.display()))
    } else {
        Config::default()
    };
    if let Err(e) = config.validate() {
        eprintln!("Config validation failed: {e}");
        exit(1);
    }
    if args.validate.is_some() {
        for spec in collect_background_specs(&config) {
            if !spec.path.exists() {
                eprintln!("Config validation failed: background path does not exist: {}", spec.path.display());
                exit(1);
            }
        }
        return;
    }
    if let Err(e) = run(config) {
        eprintln!("Compositor error: {e}");
        let mut src = e.source();
        while let Some(cause) = src {
            eprintln!("  caused by: {cause}");
            src = cause.source();
        }
        exit(1);
    }
}

fn run(config: Config) -> Result<(), Box<dyn Error>> {
    fn handle_input(state: &mut State, event: InputEvent<WinitInput>) {
        fn handle_lock_input(state: &mut State, event: InputEvent<WinitInput>) {
            match event {
                InputEvent::Keyboard { event } => {
                    state.record_lock_activity();
                    if event.state() != KeyState::Pressed {
                        if let Some(kb) = state.seat.get_keyboard() {
                            let _: ((), _) =
                                kb.input_intercept(state, event.key_code(), event.state(), |_, _, _| ());
                        }
                        return;
                    }
                    let keysym_info = if let Some(kb) = state.seat.get_keyboard() {
                        let (info, _) = kb.input_intercept(state, event.key_code(), event.state(), |_, _, handle| {
                            let modified = handle.modified_sym();
                            let raw = handle.raw_syms().into_iter().next().unwrap_or(Keysym::NoSymbol);
                            (raw, modified)
                        });
                        Some(info)
                    } else {
                        None
                    };
                    if let Some((raw, modified)) = keysym_info {
                        match raw {
                            Keysym::Return | Keysym::KP_Enter => {
                                if state.lock_input_state == LockInputState::Typing &&
                                    !state.lock_password.is_empty() {
                                    state.lock_last_keystroke = Instant::now();
                                    state.lock_input_state = LockInputState::Verifying;
                                    let (tx, rx) = channel();
                                    state.lock_verify_rx = Some(rx);
                                    spawn({
                                        let password = state.lock_password.clone();
                                        let socket_path = state.config.unlock_socket.clone();
                                        move || {
                                            let rt = RuntimeBuilder::new_current_thread().enable_all().build();
                                            let _ = tx.send(match rt {
                                                Ok(rt) => rt.block_on(async {
                                                    let mut client =
                                                        match unlock_protocol::Client::new(&socket_path).await {
                                                            Ok(c) => c,
                                                            Err(e) => {
                                                                tracing::error!(
                                                                    "Failed to connect to unlock daemon: {e}"
                                                                );
                                                                return false;
                                                            },
                                                        };
                                                    match client.send_req(CheckPassword { password }).await {
                                                        Ok(result) => result,
                                                        Err(e) => {
                                                            tracing::error!("Unlock IPC error: {e}");
                                                            false
                                                        },
                                                    }
                                                }),
                                                Err(e) => {
                                                    tracing::error!("Failed to create tokio runtime for unlock: {e}");
                                                    false
                                                },
                                            });
                                        }
                                    });
                                }
                            },
                            Keysym::BackSpace => {
                                if state.lock_input_state == LockInputState::Typing ||
                                    state.lock_input_state == LockInputState::Idle {
                                    state.lock_password.clear();
                                    state.lock_input_state = LockInputState::Idle;
                                    state.lock_circle_hidden_until = Some(Instant::now() + Duration::from_secs(10));
                                }
                            },
                            Keysym::Escape => {
                                if state.lock_input_state == LockInputState::Typing {
                                    state.lock_password.clear();
                                    state.lock_input_state = LockInputState::Idle;
                                }
                            },
                            _ => {
                                if state.lock_input_state == LockInputState::Verifying ||
                                    state.lock_input_state == LockInputState::Failed {
                                    return;
                                }
                                if let Some(c) = modified.key_char() {
                                    if !c.is_control() {
                                        state.lock_password.push(c);
                                        state.lock_input_state = LockInputState::Typing;
                                        state.lock_last_keystroke = Instant::now();
                                        state.lock_circle_hidden_until = None;
                                    }
                                }
                            },
                        }
                    }
                },
                InputEvent::PointerMotionAbsolute { event } => {
                    let pos = event.position_transformed(state.output_size);
                    state.record_lock_activity();
                    let _ = pos;
                },
                InputEvent::PointerButton { .. } => {
                    state.record_lock_activity();
                },
                _ => { },
            }
        }

        if state.screen_power_state == ScreenPowerState::Locked {
            handle_lock_input(state, event);
            return;
        }
        match event {
            InputEvent::Keyboard { event } => {
                state.record_activity();
                if let Some(kb) = state.seat.get_keyboard() {
                    kb.input::<(), _>(
                        state,
                        event.key_code(),
                        event.state(),
                        SERIAL_COUNTER.next_serial(),
                        event.time() as u32,
                        |_, _, _| FilterResult::Forward,
                    );
                }
            },
            InputEvent::PointerMotionAbsolute { event } => {
                let pos = event.position_transformed(state.output_size);
                state.record_mouse_activity(pos);
                let focus = 'focus: {
                    if let Some(origin) = state.current_window_surface_origin() {
                        if let Some(id) = state.current_window_id {
                            if let Some(mw) = state.windows.iter().find(|w| w.id == id && w.window.alive()) {
                                let origin_f: Point<f64, Logical> =
                                    Point::from((origin.x as f64, origin.y as f64));
                                if let Some((surface, surface_loc)) =
                                    mw
                                        .window
                                        .surface_under(
                                            Point::from((pos.x - origin_f.x, pos.y - origin_f.y)),
                                            WindowSurfaceType::ALL,
                                        ) {
                                    let surf_origin =
                                        Point::from(
                                            (origin_f.x + surface_loc.x as f64, origin_f.y + surface_loc.y as f64),
                                        );
                                    break 'focus Some((surface, surf_origin));
                                }
                            }
                        }
                    }
                    None
                };
                if let Some(ptr) = state.seat.get_pointer() {
                    ptr.motion(state, focus, &MotionEvent {
                        location: pos,
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time() as u32,
                    });
                    ptr.frame(state);
                }
            },
            InputEvent::PointerButton { event } => {
                state.record_activity();
                if !state.cursor_visible {
                    state.cursor_visible = true;
                }
                if let Some(ptr) = state.seat.get_pointer() {
                    ptr.button(state, &ButtonEvent {
                        button: event.button_code(),
                        state: event.state(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time() as u32,
                    });
                    ptr.frame(state);
                }
            },
            _ => { },
        }
    }

    let mut display: Display<State> = Display::new()?;
    let ipc_shared = Arc::new(Mutex::new(SharedIpcState::new()));
    let (ipc_cmd_tx, ipc_cmd_rx) = channel();
    let (mut backend, mut winit_loop) = winit::init::<GlesRenderer>()?;
    let output_size: Size<i32, Logical> = backend.window_size().to_logical(1);
    let mut state = State::new(&display, output_size, config.clone(), ipc_shared.clone(), ipc_cmd_rx);
    state.seat.add_keyboard(Default::default(), 200, 25).expect("keyboard");
    state.seat.add_pointer();
    let listener = ListeningSocket::bind(config.wayland_socket.as_str())?;
    spawn_ipc_server(config.ipc_socket.clone(), ipc_shared, ipc_cmd_tx);
    {
        let (renderer, _fb) = backend.bind()?;
        let mut bg_paths: Vec<PathBuf> = Vec::new();
        for spec in collect_background_specs(&config) {
            if !bg_paths.iter().any(|p| p == &spec.path) {
                bg_paths.push(spec.path.clone());
            }
        }
        for path in bg_paths {
            let load_result: Result<(TextureBuffer<GlesTexture>, (u32, u32)), Box<dyn Error>> = (|| {
                let rgba = ImageReader::open(&path)?.decode()?.to_rgba8();
                let (w, h) = rgba.dimensions();
                let buf =
                    TextureBuffer::from_memory(
                        renderer,
                        &rgba.into_raw(),
                        Fourcc::Abgr8888,
                        Size::<i32, Buffer>::from((w as i32, h as i32)),
                        false,
                        1,
                        Transform::Normal,
                        None,
                    )?;
                Ok((buf, (w, h)))
            })();
            match load_result {
                Ok((buf, dims)) => {
                    state.background_buffers.insert(path.clone(), (buf, dims));
                    tracing::info!("Background loaded from {}", path.display());
                },
                Err(e) => tracing::warn!("Failed to load background {}: {e}", path.display()),
            }
        }
        match renderer.compile_custom_pixel_shader(
            ROUNDED_RECT_SHADER,
            &[UniformName::new("u_color", UniformType::_4f), UniformName::new("u_radius", UniformType::_1f)],
        ) {
            Ok(prog) => state.rounded_rect_shader = Some(prog),
            Err(e) => tracing::warn!("Failed to compile rounded-rect shader: {e}"),
        }
        match renderer.compile_custom_pixel_shader(
            LOCK_SHADER,
            &[
                UniformName::new("u_color", UniformType::_4f),
                UniformName::new("u_line_width", UniformType::_1f),
                UniformName::new("u_pad", UniformType::_1f),
            ],
        ) {
            Ok(prog) => state.lock_shader = Some(prog),
            Err(e) => tracing::warn!("Failed to compile lock shader: {e}"),
        }
    }
    let mut clients = Vec::new();
    let mut prev_cursor_visible = true;
    loop {
        let status = winit_loop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                let logical: Size<i32, Logical> = size.to_logical(1);
                state.output_size = logical;
                state.output.change_current_state(Some(Mode {
                    size: Size::<i32, Physical>::from((logical.w, logical.h)),
                    refresh: 60_000,
                }), None, None, None);
                {
                    let mut layer_map = layer_map_for_output(&state.output);
                    layer_map.arrange();
                }
                if let Some(id) = state.current_window_id {
                    let params =
                        state
                            .windows
                            .iter()
                            .find(|w| w.id == id)
                            .map(|mw| state.effective_window_params_for(mw))
                            .unwrap_or_else(|| state.effective_window_params(None, None, false));
                    let content_area = state.window_content_area_for(&params);
                    if let Some(mw) = state.windows.iter().find(|w| w.id == id) {
                        if let Some(t) = mw.window.toplevel() {
                            t.with_pending_state(|s| {
                                s.size = Some(content_area.size);
                            });
                            t.send_pending_configure();
                        }
                    }
                }
            },
            WinitEvent::Input(event) => handle_input(&mut state, event),
            WinitEvent::CloseRequested => exit(0),
            _ => { },
        });
        match status {
            PumpStatus::Continue => { },
            PumpStatus::Exit(_) => return Ok(()),
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
        state.popup_manager.cleanup();
        state.process_pending();
        state.check_idle_timeouts();
        if state.cursor_visible != prev_cursor_visible {
            prev_cursor_visible = state.cursor_visible;
            backend.window().set_cursor_visible(state.cursor_visible);
        }
        if let Ok(Some(stream)) = listener.accept() {
            if let Ok(client) = display.handle().insert_client(stream, Arc::new(ClientState::default())) {
                clients.push(client);
            }
        }
        {
            let size: Size<i32, Physical> = backend.window_size();
            let damage = Rectangle::from_size(size);
            let time_ms = state.start_time.elapsed().as_millis() as u32;
            {
                let (renderer, mut framebuffer) = backend.bind()?;
                let (clear_color, elements, send_frames) = match state.screen_power_state {
                    ScreenPowerState::Active => {
                        let elems = state.render_elements(renderer);
                        (Color32F::new(0.0, 0.0, 0.0, 1.0), elems, true)
                    },
                    ScreenPowerState::Locked => {
                        if state.lock_blanked {
                            (Color32F::new(0.0, 0.0, 0.0, 1.0), Vec::new(), false)
                        } else {
                            let bg = state.config.lock_bg_color;
                            let elems = state.render_lock_elements();
                            (Color32F::new(bg[0], bg[1], bg[2], bg[3]), elems, false)
                        }
                    },
                    ScreenPowerState::Blanked | ScreenPowerState::Off => {
                        (Color32F::new(0.0, 0.0, 0.0, 1.0), Vec::new(), false)
                    },
                };
                let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
                frame.clear(clear_color, &[damage])?;
                if !elements.is_empty() {
                    let _ = draw_render_elements::<GlesRenderer, _, _>(&mut frame, 1.0, &elements, &[damage]);
                }
                let _ = frame.finish()?;
                if send_frames {
                    state.send_frames(time_ms);
                }
            }
            backend.submit(Some(&[damage]))?;
        }
    }
}
