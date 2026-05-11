use {
    aargvark::{
        Aargvark,
        vark,
    },
    mononocle::compositor::{
        config::Config,
        ipc_server::{
            SharedIpcState,
            spawn_ipc_server,
        },
        state::{
            ClientState,
            ROUNDED_RECT_SHADER,
            ScreenPowerState,
            State,
        },
    },
    smithay::{
        backend::{
            input::{
                AbsolutePositionEvent,
                Event as InputEventTrait,
                InputEvent,
                KeyboardKeyEvent,
                PointerButtonEvent,
            },
            renderer::{
                Color32F,
                Frame,
                Renderer,
                gles::{
                    GlesRenderer,
                    UniformName,
                    UniformType,
                },
                utils::draw_render_elements,
            },
            winit::{
                WinitEvent,
                WinitInput,
            },
        },
        input::pointer::{
            ButtonEvent,
            MotionEvent,
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
        path::PathBuf,
        sync::Arc,
    },
};

#[derive(Aargvark)]
struct Args {
    /// Path to the JSON configuration file.
    config: Option<PathBuf>,
    /// Validate the configuration and exit.
    validate: Option<()>,
}

fn main() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
    let args: Args = vark();
    let config: Config = if let Some(path) = args.config {
        let text =
            std::fs::read_to_string(
                &path,
            ).unwrap_or_else(|e| panic!("Failed to read config {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("Failed to parse config {}: {e}", path.display()))
    } else {
        Config::default()
    };
    if let Err(e) = config.validate() {
        eprintln!("Config validation failed: {e}");
        std::process::exit(1);
    }
    if args.validate.is_some() {
        if let Some(bg) = &config.background {
            if !bg.exists() {
                eprintln!("Config validation failed: background path does not exist: {}", bg.display());
                std::process::exit(1);
            }
        }
        println!("Config OK");
        return;
    }
    if let Err(e) = run(config) {
        eprintln!("Compositor error: {e}");
        let mut src = e.source();
        while let Some(cause) = src {
            eprintln!("  caused by: {cause}");
            src = cause.source();
        }
        std::process::exit(1);
    }
}

fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<State> = Display::new()?;
    let ipc_shared = Arc::new(std::sync::Mutex::new(SharedIpcState::new()));
    let (ipc_cmd_tx, ipc_cmd_rx) = std::sync::mpsc::channel();
    let (mut backend, mut winit_loop) = smithay::backend::winit::init::<GlesRenderer>()?;
    let output_size: Size<i32, Logical> = backend.window_size().to_logical(1);
    let mut state = State::new(&display, output_size, config.clone(), ipc_shared.clone(), ipc_cmd_rx);
    state.seat.add_keyboard(Default::default(), 200, 25).expect("keyboard");
    state.seat.add_pointer();

    // Create Wayland socket
    let listener = ListeningSocket::bind_auto("mononocle", 1 ..= 9)?;
    let socket_name = listener.socket_name().unwrap().to_string_lossy().to_string();

    // SAFETY: single-threaded at this point, no concurrent env reads
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    }
    tracing::info!("WAYLAND_DISPLAY={}", socket_name);

    // Start IPC server thread
    spawn_ipc_server(config.socket.clone(), ipc_shared, ipc_cmd_tx);

    // Bind renderer once to load background and compile shader
    {
        let (renderer, _fb) = backend.bind()?;

        if let Some(bg_path) = config.background.clone() {
            match load_background(renderer, &bg_path) {
                Ok((buf, dims)) => {
                    state.background_buffer = Some((buf, dims));
                    tracing::info!("Background loaded from {}", bg_path.display());
                },
                Err(e) => tracing::warn!("Failed to load background {}: {e}", bg_path.display()),
            }
        }

        match compile_rounded_rect_shader(renderer) {
            Ok(prog) => state.rounded_rect_shader = Some(prog),
            Err(e) => tracing::warn!("Failed to compile rounded-rect shader: {e}"),
        }
    }

    let mut clients = Vec::new();
    let mut prev_cursor_visible = true;
    loop {
        let status = winit_loop.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                let logical: Size<i32, Logical> = size.to_logical(1);
                state.output_size = logical;
                let phys = Size::<i32, Physical>::from((logical.w, logical.h));
                let mode = Mode {
                    size: phys,
                    refresh: 60_000,
                };
                state.output.change_current_state(Some(mode), None, None, None);
                // Re-arrange layer map so non_exclusive_zone reflects the new size.
                {
                    let mut layer_map = smithay::desktop::layer_map_for_output(&state.output);
                    layer_map.arrange();
                }
                if let Some(id) = state.current_window_id {
                    let params = state.windows.iter()
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
            WinitEvent::CloseRequested => std::process::exit(0),
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
        render_frame(&mut state, &mut backend)?;
    }
}

fn render_frame(
    state: &mut State,
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
) -> Result<(), Box<dyn std::error::Error>> {
    let size: Size<i32, Physical> = backend.window_size();
    let damage = Rectangle::from_size(size);
    let time_ms = state.start_time.elapsed().as_millis() as u32;
    {
        let is_active = state.screen_power_state == ScreenPowerState::Active;
        let (renderer, mut framebuffer) = backend.bind()?;
        let elements = if is_active {
            state.render_elements(renderer)
        } else {
            Vec::new()
        };
        let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
        frame.clear(Color32F::new(0.0, 0.0, 0.0, 1.0), &[damage])?;
        if is_active {
            let _ = draw_render_elements::<GlesRenderer, _, _>(&mut frame, 1.0, &elements, &[damage]);
        }
        let _ = frame.finish()?;
        if is_active {
            state.send_frames(time_ms);
        }
    }
    backend.submit(Some(&[damage]))?;
    Ok(())
}

fn load_background(
    renderer: &mut GlesRenderer,
    path: &std::path::Path,
) -> Result<
    (
        smithay::backend::renderer::element::texture::TextureBuffer<smithay::backend::renderer::gles::GlesTexture>,
        (u32, u32),
    ),
    Box<dyn std::error::Error>,
> {
    use {
        image::ImageReader,
        smithay::{
            backend::{
                allocator::Fourcc,
                renderer::element::texture::TextureBuffer,
            },
            utils::{
                Buffer,
                Transform,
            },
        },
    };

    let img = ImageReader::open(path)?.decode()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let data = rgba.into_raw();
    let size = Size::<i32, Buffer>::from((w as i32, h as i32));
    let buf =
        TextureBuffer::from_memory(renderer, &data, Fourcc::Abgr8888, size, false, 1, Transform::Normal, None)?;
    Ok((buf, (w, h)))
}

fn compile_rounded_rect_shader(
    renderer: &mut GlesRenderer,
) -> Result<smithay::backend::renderer::gles::GlesPixelProgram, Box<dyn std::error::Error>> {
    let prog = renderer.compile_custom_pixel_shader(
        ROUNDED_RECT_SHADER,
        &[
            UniformName::new("u_color", UniformType::_4f),
            UniformName::new("u_radius", UniformType::_1f),
        ],
    )?;
    Ok(prog)
}

fn handle_input(state: &mut State, event: InputEvent<WinitInput>) {
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
                    |_, _, _| smithay::input::keyboard::FilterResult::Forward,
                );
            }
        },
        InputEvent::PointerMotionAbsolute { event } => {
            let pos = event.position_transformed(state.output_size);
            state.record_mouse_activity(pos);
            let focus = pointer_focus_surface(state, pos);
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

fn pointer_focus_surface(
    state: &State,
    pos: Point<f64, Logical>,
) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<f64, Logical>)> {
    use smithay::desktop::WindowSurfaceType;
    use smithay::utils::IsAlive;

    if let Some(origin) = state.current_window_surface_origin() {
        if let Some(id) = state.current_window_id {
            if let Some(mw) = state.windows.iter().find(|w| w.id == id && w.window.alive()) {
                // Window::surface_under expects a point relative to the
                // window's wl_surface origin (not the geometry origin).
                // It internally accounts for geometry().loc when computing
                // popup offsets, so we must NOT add geo.loc to the input.
                let origin_f: Point<f64, Logical> = Point::from((
                    origin.x as f64,
                    origin.y as f64,
                ));
                let point_in_surface = Point::from((
                    pos.x - origin_f.x,
                    pos.y - origin_f.y,
                ));

                // Hit-test all window surfaces: popups, subsurfaces, and toplevel.
                // The PopupPointerGrab handles same-client focus correctly,
                // so we don't need to force focus to popup surfaces.
                if let Some((surface, surface_loc)) = mw.window.surface_under(
                    point_in_surface,
                    WindowSurfaceType::ALL,
                ) {
                    let surf_origin = Point::from((
                        origin_f.x + surface_loc.x as f64,
                        origin_f.y + surface_loc.y as f64,
                    ));
                    return Some((surface, surf_origin));
                }
            }
        }
    }
    None
}
