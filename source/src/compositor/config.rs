use {
    serde::{
        Deserialize,
        Serialize,
    },
    std::{
        collections::HashMap,
        path::PathBuf,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundSize {
    #[default]
    Cover,
    MinCover,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundSpec {
    pub path: PathBuf,
    #[serde(default = "default_background_align")]
    pub align: [f64; 2],
    #[serde(default)]
    pub size: BackgroundSize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WindowStyle {
    #[serde(default)]
    pub background: Option<BackgroundSpec>,
    #[serde(default)]
    pub border_color: Option<[f32; 4]>,
    #[serde(default)]
    pub border_thickness: Option<i32>,
    #[serde(default)]
    pub corner_radius: Option<f32>,
    #[serde(default)]
    pub fullscreen: Option<bool>,
    #[serde(default)]
    pub idle_hold: Option<IdleHoldPolicy>,
    #[serde(default)]
    pub inner_padding: Option<i32>,
    #[serde(default)]
    pub inner_padding_color: Option<[f32; 4]>,
    #[serde(default)]
    pub padding: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub cursor_hide_idle_secs: Option<f64>,
    #[serde(default)]
    pub default_style: WindowStyle,
    #[serde(default)]
    pub desktop_backgrounds: HashMap<u32, BackgroundSpec>,
    #[serde(default = "default_fullscreen_holds_idle")]
    pub fullscreen_prevents_idle: bool,
    #[serde(default = "default_ipc_socket")]
    pub ipc_socket: PathBuf,
    #[serde(default = "default_lock_bg_color")]
    pub lock_bg_color: [f32; 4],
    #[serde(default = "default_lock_fg_active_color")]
    pub lock_fg_active_color: [f32; 4],
    #[serde(default = "default_lock_fg_color")]
    pub lock_fg_color: [f32; 4],
    #[serde(default)]
    pub lock_timeout_secs: Option<f64>,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
    #[serde(default)]
    pub screen_blank_idle_secs: Option<f64>,
    #[serde(default)]
    pub screen_off_idle_secs: Option<f64>,
    #[serde(default = "default_mouse_jitter_threshold")]
    pub unidle_mouse_threshold: f64,
    #[serde(default = "default_unlock_socket")]
    pub unlock_socket: PathBuf,
    #[serde(default = "default_wayland_socket")]
    pub wayland_socket: String,
    #[serde(default)]
    pub window_rules: Vec<WindowRule>,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if let (Some(blank), Some(off)) = (self.screen_blank_idle_secs, self.screen_off_idle_secs) {
            if off < blank {
                return Err(
                    format!("display_off_timeout_secs ({off}) must be >= screen_blank_timeout_secs ({blank})"),
                );
            }
        }
        if let (Some(blank), Some(lock)) = (self.screen_blank_idle_secs, self.lock_timeout_secs) {
            if lock < blank {
                return Err(format!("lock_timeout_secs ({lock}) must be >= screen_blank_timeout_secs ({blank})"));
            }
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_style: WindowStyle::default(),
            desktop_backgrounds: HashMap::new(),
            window_rules: Vec::new(),
            wayland_socket: default_wayland_socket(),
            ipc_socket: default_ipc_socket(),
            outputs: Vec::new(),
            screen_blank_idle_secs: None,
            screen_off_idle_secs: None,
            unidle_mouse_threshold: default_mouse_jitter_threshold(),
            cursor_hide_idle_secs: None,
            fullscreen_prevents_idle: default_fullscreen_holds_idle(),
            lock_timeout_secs: None,
            lock_bg_color: default_lock_bg_color(),
            lock_fg_color: default_lock_fg_color(),
            lock_fg_active_color: default_lock_fg_active_color(),
            unlock_socket: default_unlock_socket(),
        }
    }
}

pub fn default_background_align() -> [f64; 2] {
    [0.5, 0.5]
}

pub fn default_border_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_fullscreen_holds_idle() -> bool {
    true
}

pub fn default_inner_padding_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn default_ipc_socket() -> PathBuf {
    PathBuf::from("/tmp/mononocle.sock")
}

fn default_lock_bg_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn default_lock_fg_active_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_lock_fg_color() -> [f32; 4] {
    [0.3, 0.3, 0.3, 1.0]
}

fn default_mouse_jitter_threshold() -> f64 {
    5.0
}

fn default_output_position() -> OutputPosition {
    OutputPosition::None
}

pub fn default_padding() -> i32 {
    20
}

fn default_unlock_socket() -> PathBuf {
    PathBuf::from("/run/mononocle-unlock.sock")
}

fn default_wayland_socket() -> String {
    "wayland-1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdleHoldPolicy {
    BlockHold,
    #[default]
    Default,
    ForceHold,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    #[serde(rename = "match")]
    pub criteria: OutputCriteria,
    pub desktops: Vec<u32>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_output_position")]
    pub position: OutputPosition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputCriteria {
    And(Vec<OutputCriteria>),
    Connector(String),
    Manufacturer(String),
    Model(String),
    Or(Vec<OutputCriteria>),
    Serial(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputPosition {
    Above,
    Below,
    Left,
    None,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleCriteria {
    And(Vec<RuleCriteria>),
    AppId(String),
    Or(Vec<RuleCriteria>),
    Title(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRule {
    #[serde(rename = "match")]
    pub criteria: RuleCriteria,
    #[serde(default)]
    pub style: WindowStyle,
}
