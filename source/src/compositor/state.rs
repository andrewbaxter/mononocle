use {
    crate::{
        compositor::{
            config::{
                BackgroundSize,
                Config,
                IdleHoldPolicy,
                OutputCriteria,
                OutputPosition,
                RuleCriteria,
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
                Resource,
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
                SurfaceAttributes,
                TraversalAction,
                with_states,
                with_surface_tree_downward,
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
        collections::{
            HashMap,
            HashSet,
        },
        fs::read_to_string,
        sync::{
            Arc,
            Mutex,
            atomic::{
                AtomicU64,
                Ordering,
            },
            mpsc::{
                Receiver,
                TryRecvError,
            },
        },
        time::{
            Duration,
            Instant,
        },
    },
};

render_elements!{
    pub CompElement <= GlesRenderer >;
    Surface = WaylandSurfaceRenderElement < GlesRenderer >,
    Texture = TextureRenderElement < GlesTexture >,
    Solid = SolidColorRenderElement,
    PixelShader = PixelShaderElement,
}

pub const LOCK_SHADER: &str = r#"
precision mediump float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
uniform vec4 u_color;
uniform float u_line_width;
uniform float u_pad;

float roundedBoxSDF(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

void main() {
    vec2 icon_size = size - 2.0 * u_pad;
    vec2 pixel = v_coords * size - u_pad;

    vec2 body_center = vec2(icon_size.x * 0.5, icon_size.y * 0.6858);
    vec2 body_half = vec2(icon_size.x * 0.4938, icon_size.y * 0.3096);
    float body_r = min(icon_size.y * 0.1499, min(body_half.x, body_half.y));
    float body_d = roundedBoxSDF(pixel - body_center, body_half, body_r);

    vec2 shackle_center = vec2(icon_size.x * 0.5, icon_size.y * 0.3093);
    vec2 shackle_half = vec2(icon_size.x * 0.2293, icon_size.y * 0.2573);
    float shackle_r = min(icon_size.y * 0.1751, min(shackle_half.x, shackle_half.y));
    float shackle_stroke_half = icon_size.y * 0.052;
    float shackle_d = abs(roundedBoxSDF(pixel - shackle_center, shackle_half, shackle_r)) - shackle_stroke_half;

    float union_d = min(body_d, shackle_d);
    float outline_d = abs(union_d) - u_line_width * 0.5;
    float shape_alpha = 1.0 - smoothstep(-0.5, 0.5, outline_d);
    float a = u_color.a * alpha * shape_alpha;
    gl_FragColor = vec4(u_color.rgb * a, a);
}
"#;
static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
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

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) { }

    fn initialized(&self, _client_id: ClientId) { }
}

enum CompiledCriteria {
    And(Vec<CompiledCriteria>),
    AppId(Regex),
    Or(Vec<CompiledCriteria>),
    Title(Regex),
}

impl CompiledCriteria {
    fn matches(&self, title: Option<&str>, app_id: Option<&str>) -> bool {
        match self {
            CompiledCriteria::Title(re) => title.map_or(false, |t| re.is_match(t)),
            CompiledCriteria::AppId(re) => app_id.map_or(false, |a| re.is_match(a)),
            CompiledCriteria::And(children) => children.iter().all(|c| c.matches(title, app_id)),
            CompiledCriteria::Or(children) => children.iter().any(|c| c.matches(title, app_id)),
        }
    }
}

struct CompiledOutputConfig {
    criteria: Option<CompiledOutputCriteria>,
    desktops: Vec<u32>,
    id: Option<String>,
    position: OutputPosition,
}

enum CompiledOutputCriteria {
    And(Vec<CompiledOutputCriteria>),
    Connector(Regex),
    Manufacturer(Regex),
    Model(Regex),
    Or(Vec<CompiledOutputCriteria>),
    Serial(Regex),
}

impl CompiledOutputCriteria {
    fn matches(&self, connector: &str, model: &str, manufacturer: &str, serial: &str) -> bool {
        match self {
            CompiledOutputCriteria::Connector(re) => re.is_match(connector),
            CompiledOutputCriteria::Model(re) => re.is_match(model),
            CompiledOutputCriteria::Manufacturer(re) => re.is_match(manufacturer),
            CompiledOutputCriteria::Serial(re) => re.is_match(serial),
            CompiledOutputCriteria::And(children) => {
                children.iter().all(|c| c.matches(connector, model, manufacturer, serial))
            },
            CompiledOutputCriteria::Or(children) => {
                children.iter().any(|c| c.matches(connector, model, manufacturer, serial))
            },
        }
    }
}

struct CompiledRule {
    criteria: Option<CompiledCriteria>,
    rule: WindowRule,
}

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

pub struct EffectiveWindowParams {
    pub border_color: [f32; 4],
    pub border_thickness: i32,
    pub corner_radius: f32,
    pub fullscreen: bool,
    pub idle_hold: IdleHoldPolicy,
    pub inner_padding: i32,
    pub inner_padding_color: [f32; 4],
    pub padding: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockInputState {
    Failed,
    Idle,
    Typing,
    Verifying,
}

pub struct ManagedWindow {
    pub desktop: u32,
    pub fullscreen: bool,
    pub id: u64,
    pub window: Window,
}

impl ManagedWindow {
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

pub fn next_window_id() -> u64 {
    NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed)
}

pub type OutputIndex = usize;

pub struct OutputState {
    pub current_desktop: u32,
    pub current_window_id: Option<u64>,
    pub desktops: Vec<u32>,
    pub id: Option<String>,
    pub position: OutputPosition,
}

pub fn pid_ancestors(pid: u32) -> PidAncestors {
    PidAncestors { next: Some(pid) }
}

pub struct PidAncestors {
    next: Option<u32>,
}

impl Iterator for PidAncestors {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        fn parent_pid(pid: u32) -> Option<u32> {
            let status = read_to_string(format!("/proc/{pid}/status")).ok()?;
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("PPid:") {
                    return rest.trim().parse().ok();
                }
            }
            None
        }

        let pid = self.next?;
        if pid == 0 {
            self.next = None;
            return None;
        }
        self.next = if pid <= 1 {
            None
        } else {
            parent_pid(pid)
        };
        Some(pid)
    }
}

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
            let elem =
                PixelShaderElement::new(
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
    let buf = SolidColorBuffer::new(rect.size, Color32F::new(color[0], color[1], color[2], color[3]));
    let elem =
        SolidColorRenderElement::from_buffer(
            &buf,
            Point::<i32, Physical>::from((rect.loc.x, rect.loc.y)),
            1.0f64,
            1.0,
            Kind::Unspecified,
        );
    elements.push(CompElement::Solid(elem));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPowerState {
    Active,
    Blanked,
    Locked,
    Off,
}

pub struct State {
    pub activity_mouse_pos: Point<f64, Logical>,
    pub background_buffer: Option<(TextureBuffer<GlesTexture>, (u32, u32))>,
    compiled_output_configs: Vec<CompiledOutputConfig>,
    compiled_rules: Vec<CompiledRule>,
    pub compositor_state: CompositorState,
    pub config: Config,
    pub current_desktop: u32,
    pub current_output: OutputIndex,
    pub current_window_id: Option<u64>,
    pub cursor_visible: bool,
    pub data_device_state: DataDeviceState,
    pub display_handle: DisplayHandle,
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub idle_inhibit_surfaces: HashSet<WlSurface>,
    pub ipc_rx: Receiver<IpcCommand>,
    pub ipc_shared: Arc<Mutex<SharedIpcState>>,
    pub last_activity: Instant,
    pub last_mouse_activity: Instant,
    pub layer_shell_state: WlrLayerShellState,
    pub layer_surfaces: Vec<DesktopLayerSurface>,
    pub lock_blanked: bool,
    pub lock_circle_hidden_until: Option<Instant>,
    pub lock_input_state: LockInputState,
    pub lock_last_keystroke: Instant,
    pub lock_password: String,
    pub lock_shader: Option<GlesPixelProgram>,
    pub lock_state_changed: Instant,
    pub lock_verify_rx: Option<Receiver<bool>>,
    pub output: Output,
    pub output_manager_state: OutputManagerState,
    pub output_size: Size<i32, Logical>,
    pub output_states: Vec<OutputState>,
    pub pid_desktop: HashMap<u32, u32>,
    pub popup_manager: PopupManager,
    pub rounded_rect_shader: Option<GlesPixelProgram>,
    pub screen_power_state: ScreenPowerState,
    pub seat: Seat<Self>,
    pub seat_state: SeatState<Self>,
    pub shm_state: ShmState,
    pub start_time: Instant,
    pub warp_mouse_needed: bool,
    pub windows: Vec<ManagedWindow>,
    pub xdg_shell_state: XdgShellState,
}

impl State {
    pub fn associate_current_window_pid(&mut self) {
        let Some(id) = self.current_window_id else {
            return
        };
        let desktop = self.current_desktop;
        let pid =
            self
                .windows
                .iter()
                .find(|w| w.id == id && w.window.alive())
                .and_then(|mw| mw.window.toplevel())
                .and_then(|t| self.pid_for_surface(t.wl_surface()));
        if let Some(pid) = pid {
            self.associate_pid_tree_with_desktop(pid, desktop);
        }
    }

    pub fn associate_pid_tree_with_desktop(&mut self, pid: u32, desktop: u32) {
        for ancestor in pid_ancestors(pid) {
            self.pid_desktop.insert(ancestor, desktop);
        }
    }

    pub fn attach_output(
        &mut self,
        connector: &str,
        model: &str,
        manufacturer: &str,
        serial: &str,
    ) -> Option<OutputIndex> {
        let config_idx = self.match_output_config(connector, model, manufacturer, serial)?;
        let cfg = &self.compiled_output_configs[config_idx];
        let id = cfg.id.clone();
        let desktops = cfg.desktops.clone();
        let position = cfg.position.clone();
        self.output_states[0].desktops.retain(|d| !desktops.contains(d));
        let first_desktop = desktops.first().copied().unwrap_or(0);
        let idx = self.output_states.len();
        self.output_states.push(OutputState {
            id,
            desktops,
            current_desktop: first_desktop,
            current_window_id: self
                .windows
                .iter()
                .find(|w| w.desktop == first_desktop && w.window.alive())
                .map(|w| w.id),
            position,
        });
        tracing::info!("Output matched config: connector={connector}, assigned output index {idx}");
        Some(idx)
    }

    pub fn check_idle_timeouts(&mut self) {
        if let Some(cursor_secs) = self.config.cursor_hide_idle_secs {
            if self.cursor_visible && self.last_mouse_activity.elapsed() >= Duration::from_secs_f64(cursor_secs) {
                self.cursor_visible = false;
                tracing::debug!("Cursor hidden after {cursor_secs}s mouse idle");
            }
        }
        if let Some(ref rx) = self.lock_verify_rx {
            match rx.try_recv() {
                Ok(true) => {
                    tracing::info!("Unlock successful");
                    self.screen_power_state = ScreenPowerState::Active;
                    self.lock_input_state = LockInputState::Idle;
                    self.lock_password.clear();
                    self.lock_verify_rx = None;
                    self.lock_blanked = false;
                    self.last_activity = Instant::now();
                    return;
                },
                Ok(false) => {
                    tracing::debug!("Unlock failed");
                    self.lock_input_state = LockInputState::Failed;
                    self.lock_state_changed = Instant::now();
                    self.lock_password.clear();
                    self.lock_verify_rx = None;
                },
                Err(TryRecvError::Empty) => { },
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("Unlock verify channel disconnected");
                    self.lock_input_state = LockInputState::Failed;
                    self.lock_state_changed = Instant::now();
                    self.lock_password.clear();
                    self.lock_verify_rx = None;
                },
            }
        }
        if self.lock_input_state == LockInputState::Failed &&
            self.lock_state_changed.elapsed() >= Duration::from_secs(2) {
            self.lock_input_state = LockInputState::Idle;
        }
        if self.screen_power_state == ScreenPowerState::Locked {
            if !self.lock_blanked {
                if let Some(blank_secs) = self.config.screen_blank_idle_secs {
                    if self.last_activity.elapsed() >= Duration::from_secs_f64(blank_secs) {
                        self.lock_blanked = true;
                        tracing::debug!("Lock screen blanked after {blank_secs}s idle");
                    }
                }
            }
            return;
        }
        if self.is_idle_held() {
            return;
        }
        let elapsed = self.last_activity.elapsed();
        if let Some(lock_secs) = self.config.lock_timeout_secs {
            if elapsed >= Duration::from_secs_f64(lock_secs) {
                if self.screen_power_state != ScreenPowerState::Locked {
                    self.screen_power_state = ScreenPowerState::Locked;
                    self.lock_input_state = LockInputState::Idle;
                    self.lock_password.clear();
                    self.lock_blanked = false;
                    self.cursor_visible = false;
                    tracing::debug!("Screen locked after {lock_secs}s idle");
                }
                return;
            }
        }
        if let Some(off_secs) = self.config.screen_off_idle_secs {
            if elapsed >= Duration::from_secs_f64(off_secs) {
                if self.screen_power_state != ScreenPowerState::Off {
                    self.screen_power_state = ScreenPowerState::Off;
                    self.cursor_visible = false;
                    tracing::debug!("Display off after {off_secs}s idle");
                }
                return;
            }
        }
        if let Some(blank_secs) = self.config.screen_blank_idle_secs {
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

    pub fn compute_warped_mouse_pos(&self, current_pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let w = self.output_size.w as f64;
        let h = self.output_size.h as f64;
        if w <= 0.0 || h <= 0.0 {
            return current_pos;
        }
        let frac_x = (current_pos.x / w).clamp(0.0, 1.0);
        let frac_y = (current_pos.y / h).clamp(0.0, 1.0);
        Point::from((frac_x * w, frac_y * h))
    }

    pub fn current_window_surface_origin(&self) -> Option<Point<i32, Logical>> {
        let id = self.current_window_id?;
        let mw = self.windows.iter().find(|w| w.id == id && w.window.alive())?;
        let params = self.effective_window_params_for(mw);
        let content_area = self.window_content_area_for(&params);
        let geo = mw.window.geometry();
        if geo.size.w > 0 && geo.size.h > 0 && (geo.size.w < content_area.size.w || geo.size.h < content_area.size.h) {
            let cx = content_area.loc.x + (content_area.size.w - geo.size.w) / 2;
            let cy = content_area.loc.y + (content_area.size.h - geo.size.h) / 2;
            Some(Point::from((cx - geo.loc.x, cy - geo.loc.y)))
        } else {
            Some(Point::from((content_area.loc.x, content_area.loc.y)))
        }
    }

    pub fn desktop_for_pid(&self, pid: u32) -> Option<u32> {
        for ancestor in pid_ancestors(pid) {
            if let Some(&desktop) = self.pid_desktop.get(&ancestor) {
                return Some(desktop);
            }
        }
        None
    }

    pub fn effective_window_params(
        &self,
        title: Option<&str>,
        app_id: Option<&str>,
        is_fullscreen: bool,
    ) -> EffectiveWindowParams {
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
            let matched = cr.criteria.as_ref().map_or(false, |c| c.matches(title, app_id));
            if matched {
                if let Some(v) = cr.rule.padding {
                    params.padding = v;
                }
                if let Some(v) = cr.rule.corner_radius {
                    params.corner_radius = v;
                }
                if let Some(v) = cr.rule.inner_padding {
                    params.inner_padding = v;
                }
                if let Some(v) = cr.rule.inner_padding_color {
                    params.inner_padding_color = v;
                }
                if let Some(v) = cr.rule.border_thickness {
                    params.border_thickness = v;
                }
                if let Some(v) = cr.rule.border_color {
                    params.border_color = v;
                }
                if let Some(v) = cr.rule.fullscreen {
                    params.fullscreen = v;
                }
                if let Some(ref v) = cr.rule.idle_hold {
                    params.idle_hold = v.clone();
                }
                break;
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

    pub fn effective_window_params_for(&self, mw: &ManagedWindow) -> EffectiveWindowParams {
        self.effective_window_params(mw.title().as_deref(), mw.app_id().as_deref(), mw.fullscreen)
    }

    pub fn global_current_window_id(&self) -> Option<u64> {
        self.output_states.get(self.current_output).and_then(|os| os.current_window_id)
    }

    pub fn is_idle_held(&self) -> bool {
        let Some(id) = self.current_window_id else {
            return false
        };
        let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) else {
            return false
        };
        let params = self.effective_window_params_for(mw);
        match params.idle_hold {
            IdleHoldPolicy::ForceHold => return true,
            IdleHoldPolicy::BlockHold => return false,
            IdleHoldPolicy::Default => { },
        }
        if self.config.fullscreen_prevents_idle && params.fullscreen {
            return true;
        }
        if let Some(t) = mw.window.toplevel() {
            if self.idle_inhibit_surfaces.contains(t.wl_surface()) {
                return true;
            }
        }
        false
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

    pub fn match_output_config(
        &self,
        connector: &str,
        model: &str,
        manufacturer: &str,
        serial: &str,
    ) -> Option<usize> {
        for (i, cfg) in self.compiled_output_configs.iter().enumerate() {
            if let Some(ref criteria) = cfg.criteria {
                if criteria.matches(connector, model, manufacturer, serial) {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn new(
        display: &Display<Self>,
        output_size: Size<i32, Logical>,
        config: Config,
        ipc_shared: Arc<Mutex<SharedIpcState>>,
        ipc_rx: Receiver<IpcCommand>,
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
        let mode = Mode {
            size: Size::<i32, Physical>::from((output_size.w, output_size.h)),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some(Point::from((0, 0))));
        output.set_preferred(mode);
        output.create_global::<Self>(&dh);
        let compiled_rules = {
            fn compile_criteria(criteria: &RuleCriteria) -> Option<CompiledCriteria> {
                match criteria {
                    RuleCriteria::Title(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid title regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledCriteria::Title),
                    RuleCriteria::AppId(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid app_id regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledCriteria::AppId),
                    RuleCriteria::And(children) => {
                        let compiled: Vec<_> = children.iter().filter_map(compile_criteria).collect();
                        if compiled.len() == children.len() {
                            Some(CompiledCriteria::And(compiled))
                        } else {
                            None
                        }
                    },
                    RuleCriteria::Or(children) => {
                        let compiled: Vec<_> = children.iter().filter_map(compile_criteria).collect();
                        if compiled.len() == children.len() {
                            Some(CompiledCriteria::Or(compiled))
                        } else {
                            None
                        }
                    },
                }
            }

            config.window_rules.iter().map(|rule| {
                let criteria = compile_criteria(&rule.criteria);
                CompiledRule {
                    criteria,
                    rule: rule.clone(),
                }
            }).collect::<Vec<_>>()
        };
        let compiled_output_configs = {
            fn compile_output_criteria(criteria: &OutputCriteria) -> Option<CompiledOutputCriteria> {
                match criteria {
                    OutputCriteria::Connector(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid connector regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledOutputCriteria::Connector),
                    OutputCriteria::Model(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid model regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledOutputCriteria::Model),
                    OutputCriteria::Manufacturer(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid manufacturer regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledOutputCriteria::Manufacturer),
                    OutputCriteria::Serial(pat) => Regex::new(pat)
                        .map_err(|e| tracing::warn!("Invalid serial regex {:?}: {e}", pat))
                        .ok()
                        .map(CompiledOutputCriteria::Serial),
                    OutputCriteria::And(children) => {
                        let compiled: Vec<_> = children.iter().filter_map(compile_output_criteria).collect();
                        if compiled.len() == children.len() {
                            Some(CompiledOutputCriteria::And(compiled))
                        } else {
                            None
                        }
                    },
                    OutputCriteria::Or(children) => {
                        let compiled: Vec<_> = children.iter().filter_map(compile_output_criteria).collect();
                        if compiled.len() == children.len() {
                            Some(CompiledOutputCriteria::Or(compiled))
                        } else {
                            None
                        }
                    },
                }
            }

            config.outputs.iter().map(|cfg| {
                let criteria = compile_output_criteria(&cfg.criteria);
                CompiledOutputConfig {
                    id: cfg.id.clone(),
                    criteria,
                    desktops: cfg.desktops.clone(),
                    position: cfg.position.clone(),
                }
            }).collect::<Vec<_>>()
        };
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
            pid_desktop: HashMap::new(),
            background_buffer: None,
            rounded_rect_shader: None,
            lock_shader: None,
            ipc_shared,
            ipc_rx,
            last_activity: now,
            last_mouse_activity: now,
            activity_mouse_pos: Point::from((0.0, 0.0)),
            screen_power_state: ScreenPowerState::Active,
            cursor_visible: true,
            idle_inhibit_surfaces: HashSet::new(),
            lock_input_state: LockInputState::Idle,
            lock_password: String::new(),
            lock_verify_rx: None,
            lock_state_changed: now,
            lock_blanked: false,
            lock_last_keystroke: now,
            lock_circle_hidden_until: None,
            output_states: vec![OutputState {
                id: None,
                desktops: (0 .. config.desktops).collect(),
                current_desktop: 0,
                current_window_id: None,
                position: OutputPosition::None,
            }],
            current_output: 0,
            warp_mouse_needed: false,
            compiled_output_configs,
            config,
            compiled_rules,
            display_handle: dh,
            start_time: now,
        }
    }

    pub fn output_for_desktop(&self, desktop: u32) -> OutputIndex {
        for (i, os) in self.output_states.iter().enumerate() {
            if os.desktops.contains(&desktop) {
                return i;
            }
        }
        0
    }

    pub fn output_index_by_id(&self, id: &str) -> Option<OutputIndex> {
        self.output_states.iter().position(|os| os.id.as_deref() == Some(id))
    }

    pub fn pid_for_surface(&self, surface: &WlSurface) -> Option<u32> {
        let dh = DisplayHandle::from(surface.handle().upgrade()?);
        Some(dh.get_client(surface.id()).ok()?.get_credentials(&dh).ok()?.pid as u32)
    }

    pub fn process_pending(&mut self) {
        let dead: Vec<u64> = self.windows.iter().filter(|w| !w.window.alive()).map(|w| w.id).collect();
        for id in dead {
            self.remove_window(id);
        }
        self.layer_surfaces.retain(|s| s.alive());
        self.idle_inhibit_surfaces.retain(|s| s.alive());
        while let Ok(cmd) = self.ipc_rx.try_recv() {
            match cmd {
                IpcCommand::ShowDesktop { desktop, output } => {
                    self.show_desktop(desktop, output.as_deref());
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
                IpcCommand::SetDesktop { pid, desktop } => {
                    let d = desktop.unwrap_or(self.current_desktop);
                    self.associate_pid_tree_with_desktop(pid, d);
                },
            }
        }
    }

    pub fn push_event(&self, event: WindowEvent) {
        let shared = self.ipc_shared.lock().unwrap();
        let _ = shared.event_tx.send(event);
    }

    pub fn reapply_window_rules(&mut self, wid: u64) {
        let Some(mw) = self.windows.iter().find(|w| w.id == wid) else {
            return
        };
        let params = self.effective_window_params_for(mw);
        let old_fullscreen = mw.fullscreen;
        let new_fullscreen = params.fullscreen;
        if old_fullscreen != new_fullscreen {
            if let Some(mw) = self.windows.iter_mut().find(|w| w.id == wid) {
                mw.fullscreen = new_fullscreen;
            }
            let content_area = self.window_content_area_for(&params);
            if let Some(mw) = self.windows.iter().find(|w| w.id == wid) {
                if let Some(t) = mw.window.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = Some(content_area.size);
                        if new_fullscreen {
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

    pub fn record_activity(&mut self) {
        self.last_activity = Instant::now();
        match self.screen_power_state {
            ScreenPowerState::Active => { },
            ScreenPowerState::Locked => { },
            ScreenPowerState::Blanked | ScreenPowerState::Off => {
                self.screen_power_state = ScreenPowerState::Active;
                tracing::debug!("Screen woke up");
            },
        }
    }

    pub fn record_lock_activity(&mut self) {
        self.last_activity = Instant::now();
        if self.screen_power_state != ScreenPowerState::Locked {
            self.screen_power_state = ScreenPowerState::Locked;
        }
        if self.lock_blanked {
            self.lock_blanked = false;
            tracing::debug!("Lock screen unblanked");
        }
    }

    pub fn record_mouse_activity(&mut self, pos: Point<f64, Logical>) -> bool {
        let dx = pos.x - self.activity_mouse_pos.x;
        let dy = pos.y - self.activity_mouse_pos.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist >= self.config.unidle_mouse_threshold {
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

    fn remove_window(&mut self, id: u64) {
        let window_desktop = self.windows.iter().find(|w| w.id == id).map(|w| w.desktop);
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
                let params =
                    self
                        .windows
                        .iter()
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
        if let Some(desktop) = window_desktop {
            let output_idx = self.output_for_desktop(desktop);
            if let Some(os) = self.output_states.get_mut(output_idx) {
                if os.current_window_id == Some(id) {
                    let next =
                        self
                            .windows
                            .iter()
                            .find(|w| w.id != id && w.desktop == os.current_desktop && w.window.alive())
                            .map(|w| w.id);
                    os.current_window_id = next;
                }
            }
        }
        self.windows.retain(|w| w.id != id);
        self.push_event(WindowEvent::WindowDeleted { id });
        self.sync_ipc_windows();
    }

    pub fn render_elements(&self, renderer: &mut GlesRenderer) -> Vec<CompElement> {
        let mut elements: Vec<CompElement> = Vec::new();
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
        if let Some(id) = self.current_window_id {
            if let Some(mw) = self.windows.iter().find(|w| w.id == id && w.window.alive()) {
                let params = self.effective_window_params_for(mw);
                let outer_rect = self.window_outer_area_for(&params);
                let content_area = self.window_content_area_for(&params);
                let radius = params.corner_radius;
                let shader = self.rounded_rect_shader.as_ref();
                if let Some(toplevel) = mw.window.toplevel() {
                    let geo = mw.window.geometry();
                    let (origin_x, origin_y) =
                        if geo.size.w > 0 && geo.size.h > 0 &&
                            (geo.size.w < content_area.size.w || geo.size.h < content_area.size.h) {
                            let cx = content_area.loc.x + (content_area.size.w - geo.size.w) / 2;
                            let cy = content_area.loc.y + (content_area.size.h - geo.size.h) / 2;
                            (cx - geo.loc.x, cy - geo.loc.y)
                        } else {
                            (content_area.loc.x, content_area.loc.y)
                        };
                    for (popup, popup_offset) in PopupManager::popups_for_surface(toplevel.wl_surface()) {
                        let popup_geo = popup.geometry();
                        let popup_loc =
                            Point::<i32, Physical>::from(
                                (
                                    origin_x + geo.loc.x + popup_offset.x - popup_geo.loc.x,
                                    origin_y + geo.loc.y + popup_offset.y - popup_geo.loc.y,
                                ),
                            );
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
                if params.inner_padding > 0 {
                    let amount = params.border_thickness;
                    let ip_rect =
                        Rectangle::new(
                            Point::from((outer_rect.loc.x + amount, outer_rect.loc.y + amount)),
                            Size::from(
                                ((outer_rect.size.w - 2 * amount).max(1), (outer_rect.size.h - 2 * amount).max(1)),
                            ),
                        );
                    let ip_radius = (radius - params.border_thickness as f32).max(0.0);
                    push_colored_rect(&mut elements, ip_rect, params.inner_padding_color, ip_radius, shader);
                }
                if params.border_thickness > 0 {
                    push_colored_rect(&mut elements, outer_rect, params.border_color, radius, shader);
                }
            }
        }
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

    pub fn render_lock_elements(&self) -> Vec<CompElement> {
        let mut elements: Vec<CompElement> = Vec::new();
        if let Some(until) = self.lock_circle_hidden_until {
            if Instant::now() < until {
                return elements;
            }
        }
        let color = match self.lock_input_state {
            LockInputState::Failed => [1.0, 0.3, 0.3, 1.0],
            _ => {
                if self.lock_last_keystroke.elapsed() < Duration::from_millis(10) {
                    self.config.lock_fg_active_color
                } else {
                    self.config.lock_fg_color
                }
            },
        };
        const ASPECT: f32 = 33.943172 / 44.452019;
        let icon_h = self.output_size.h as f32 / 20.0;
        let icon_w = icon_h * ASPECT;
        let pad = 4;
        let elem_w = icon_w as i32 + pad * 2;
        let elem_h = icon_h as i32 + pad * 2;
        let cx = self.output_size.w / 2;
        let cy = self.output_size.h / 2;
        let lock_rect =
            Rectangle::new(Point::from((cx - elem_w / 2, cy - elem_h / 2)), Size::from((elem_w, elem_h)));
        if let Some(prog) = self.lock_shader.as_ref() {
            let elem =
                PixelShaderElement::new(
                    prog.clone(),
                    lock_rect,
                    None,
                    1.0,
                    vec![
                        Uniform::new("u_color", UniformValue::_4f(color[0], color[1], color[2], color[3])),
                        Uniform::new("u_line_width", UniformValue::_1f(3.0)),
                        Uniform::new("u_pad", UniformValue::_1f(pad as f32)),
                    ],
                    Kind::Unspecified,
                );
            elements.push(CompElement::PixelShader(elem));
        }
        elements
    }

    pub fn send_frames(&self, time_ms: u32) {
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

    pub fn show_desktop(&mut self, desktop: u32, output: Option<&str>) {
        let target_output = self.output_for_desktop(desktop);
        if let Some(id) = output {
            match self.output_index_by_id(id) {
                None => {
                    tracing::warn!("show_desktop: unknown output id {:?}", id);
                    return;
                },
                Some(idx) if idx != target_output => {
                    tracing::warn!("show_desktop: desktop {desktop} is not on output {:?}", id);
                    return;
                },
                Some(_) => { },
            }
        }
        let needs_warp = target_output != self.current_output;
        if needs_warp {
            if let Some(os) = self.output_states.get(target_output) {
                if matches!(os.position, OutputPosition::None) {
                    tracing::debug!("Desktop {desktop} is on unreachable output {target_output}");
                    return;
                }
            }
        }
        let first = self.windows.iter().find(|w| w.desktop == desktop && w.window.alive()).map(|w| w.id);
        if let Some(os) = self.output_states.get_mut(target_output) {
            if os.current_desktop == desktop && !needs_warp {
                return;
            }
            os.current_desktop = desktop;
            os.current_window_id = first;
        }
        let prev = self.current_window_id;
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
        if needs_warp {
            self.current_output = target_output;
            self.warp_mouse_needed = true;
        }
        self.current_desktop = desktop;
        self.current_window_id = first;
        if let Some(new_id) = first {
            let params =
                self
                    .windows
                    .iter()
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
        self.associate_current_window_pid();
        self.push_event(WindowEvent::ShownDesktopChanged { desktop });
        self.push_event(WindowEvent::ShownWindowChanged { window_id: first });
    }

    pub fn show_window(&mut self, id: u64) {
        if self.current_window_id == Some(id) {
            return;
        }
        let prev = self.current_window_id;
        self.current_window_id = Some(id);
        if let Some(w) = self.windows.iter().find(|w| w.id == id) {
            let desktop = w.desktop;
            self.current_desktop = desktop;
            let target_output = self.output_for_desktop(desktop);
            if let Some(os) = self.output_states.get_mut(target_output) {
                os.current_desktop = desktop;
                os.current_window_id = Some(id);
            }
            if target_output != self.current_output {
                if let Some(os) = self.output_states.get(target_output) {
                    if !matches!(os.position, OutputPosition::None) {
                        self.current_output = target_output;
                        self.warp_mouse_needed = true;
                    }
                }
            }
        }
        let params =
            self
                .windows
                .iter()
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
        self.associate_current_window_pid();
        self.push_event(WindowEvent::ShownWindowChanged { window_id: Some(id) });
        self.push_event(WindowEvent::ShownDesktopChanged { desktop: self.current_desktop });
    }

    pub fn sync_ipc_windows(&self) {
        let windows: Vec<WindowInfo> =
            self
                .windows
                .iter()
                .filter(|w| w.window.alive())
                .map(|w| w.to_info(self.current_window_id))
                .collect();
        let lock_inhibited = self.is_idle_held();
        let mut shared = self.ipc_shared.lock().unwrap();
        shared.windows = windows;
        shared.current_window_id = self.current_window_id;
        shared.current_desktop = self.current_desktop;
        if shared.lock_inhibited != lock_inhibited {
            shared.lock_inhibited = lock_inhibited;
            let _ = shared.event_tx.send(WindowEvent::LockInhibitedChanged { lock_inhibited });
        }
    }

    pub fn toggle_fullscreen(&mut self, id: Option<u64>) {
        let target = id.or(self.current_window_id);
        if let Some(wid) = target {
            if let Some(mw) = self.windows.iter_mut().find(|w| w.id == wid) {
                mw.fullscreen = !mw.fullscreen;
            }
            let params =
                self
                    .windows
                    .iter()
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

    pub fn window_content_area_for(&self, params: &EffectiveWindowParams) -> Rectangle<i32, Logical> {
        let outer = self.window_outer_area_for(params);
        let inset = params.border_thickness + params.inner_padding;
        Rectangle::new(
            Point::from((outer.loc.x + inset, outer.loc.y + inset)),
            Size::from(((outer.size.w - 2 * inset).max(1), (outer.size.h - 2 * inset).max(1))),
        )
    }

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
}
