use {
    serde::{
        Deserialize,
        Serialize,
    },
    std::path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundSize {
    /// Scale image to cover the full screen (crop if needed).
    #[default]
    Cover,
    /// Use original image size unless it would be smaller than the screen in
    /// any dimension, in which case fall back to cover.
    MinCover,
}

/// Controls whether a window holds (inhibits) screen blank/off.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdleHoldPolicy {
    /// The window controls its own hold via the wayland idle-inhibit protocol.
    #[default]
    Default,
    /// Always hold (prevent blank/off) when this window is current.
    ForceHold,
    /// Never hold, even if the window requests it.
    BlockHold,
}

/// Boolean tree for matching windows by title and/or app_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleCriteria {
    /// Matches if the window title matches this regex.
    Title(String),
    /// Matches if the app_id matches this regex.
    AppId(String),
    /// Matches if all sub-criteria match.
    And(Vec<RuleCriteria>),
    /// Matches if any sub-criterion matches.
    Or(Vec<RuleCriteria>),
}

/// Boolean tree for matching outputs by connector, model, manufacturer, or serial.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputCriteria {
    /// Matches if the connector name matches this regex (e.g. "HDMI-A-1").
    Connector(String),
    /// Matches if the monitor model matches this regex.
    Model(String),
    /// Matches if the manufacturer matches this regex.
    Manufacturer(String),
    /// Matches if the serial number matches this regex.
    Serial(String),
    /// Matches if all sub-criteria match.
    And(Vec<OutputCriteria>),
    /// Matches if any sub-criterion matches.
    Or(Vec<OutputCriteria>),
}

/// Positional relation of a secondary output to the main output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputPosition {
    /// To the left of the main output.
    Left,
    /// To the right of the main output.
    Right,
    /// Above the main output.
    Above,
    /// Below the main output.
    Below,
    /// No positional relation — mouse cannot move to this output.
    None,
}

/// Configuration for a named/matched output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Boolean tree of matching criteria for this output.
    #[serde(rename = "match")]
    pub criteria: OutputCriteria,
    /// Desktops assigned to this output.
    pub desktops: Vec<u32>,
    /// Positional relation to the main output.
    #[serde(default = "default_output_position")]
    pub position: OutputPosition,
}

fn default_output_position() -> OutputPosition {
    OutputPosition::None
}

/// A window rule that overrides decoration parameters for matching windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowRule {
    /// Boolean tree of matching criteria.
    #[serde(rename = "match")]
    pub criteria: RuleCriteria,
    /// Override outer padding in logical pixels.
    pub padding: Option<i32>,
    /// Override corner radius in logical pixels.
    pub corner_radius: Option<f32>,
    /// Override inner padding size in logical pixels.
    pub inner_padding: Option<i32>,
    /// Override inner padding color as [r, g, b, a] in 0..1.
    pub inner_padding_color: Option<[f32; 4]>,
    /// Override border thickness in logical pixels.
    pub border_thickness: Option<i32>,
    /// Override border color as [r, g, b, a] in 0..1.
    pub border_color: Option<[f32; 4]>,
    /// Start the window in fullscreen mode (no padding/border/decorations).
    pub fullscreen: Option<bool>,
    /// Override idle hold policy for this window.
    pub idle_hold: Option<IdleHoldPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to background image (PNG or JPEG).
    pub background: Option<PathBuf>,
    /// Background alignment as [x, y] fractions in 0..1. Default: [0.5, 0.5] (center).
    #[serde(default = "default_background_align")]
    pub background_align: [f64; 2],
    /// Background size mode.
    #[serde(default)]
    pub background_size: BackgroundSize,
    /// Outer padding around windows in logical pixels.
    #[serde(default = "default_padding")]
    pub padding: i32,
    /// Window corner radius in logical pixels.
    #[serde(default)]
    pub corner_radius: f32,
    /// Inner padding around window content in logical pixels.
    #[serde(default)]
    pub inner_padding: i32,
    /// Inner padding color as [r, g, b, a] in 0..1.
    #[serde(default = "default_inner_padding_color")]
    pub inner_padding_color: [f32; 4],
    /// Border thickness in logical pixels.
    #[serde(default)]
    pub border_thickness: i32,
    /// Border color as [r, g, b, a] in 0..1.
    #[serde(default = "default_border_color")]
    pub border_color: [f32; 4],
    /// Window rules applied in order; the first matching rule is applied.
    #[serde(default)]
    pub window_rules: Vec<WindowRule>,
    /// Path for the IPC Unix socket.
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
    /// Number of virtual desktops.
    #[serde(default = "default_desktops")]
    pub desktops: u32,
    /// Output configurations: match outputs and assign desktops/positions.
    /// The main output is chosen from any initially non-matching output.
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
    /// Seconds of inactivity before the screen blanks (renders black). None = disabled.
    #[serde(default)]
    pub screen_blank_timeout_secs: Option<f64>,
    /// Seconds of inactivity before the display turns off. Must be >= screen_blank_timeout_secs
    /// when both are set. None = disabled.
    #[serde(default)]
    pub display_off_timeout_secs: Option<f64>,
    /// Minimum mouse movement distance (in logical pixels) from the position at idle
    /// start to count as real activity (avoids jitter waking the screen). Default: 5.
    #[serde(default = "default_mouse_jitter_threshold")]
    pub mouse_jitter_threshold: f64,
    /// Seconds of mouse inactivity before the cursor is hidden. None = disabled.
    #[serde(default)]
    pub cursor_hide_timeout_secs: Option<f64>,
    /// Whether fullscreen windows automatically hold (inhibit) screen blank/off.
    /// Default: true.
    #[serde(default = "default_fullscreen_holds_idle")]
    pub fullscreen_holds_idle: bool,
    /// Seconds of inactivity before the screen locks. Must be >= screen_blank_timeout_secs
    /// when both are set. None = disabled.
    #[serde(default)]
    pub lock_timeout_secs: Option<f64>,
    /// Lock screen background color as [r, g, b, a] in 0..1.
    #[serde(default = "default_lock_bg_color")]
    pub lock_bg_color: [f32; 4],
    /// Lock screen foreground (inactive/idle) color as [r, g, b, a] in 0..1.
    #[serde(default = "default_lock_fg_color")]
    pub lock_fg_color: [f32; 4],
    /// Lock screen foreground (active/typing) color as [r, g, b, a] in 0..1.
    #[serde(default = "default_lock_fg_active_color")]
    pub lock_fg_active_color: [f32; 4],
    /// Path for the unlock daemon IPC Unix socket.
    #[serde(default = "default_unlock_socket")]
    pub unlock_socket: PathBuf,
}

fn default_background_align() -> [f64; 2] {
    [0.5, 0.5]
}

fn default_padding() -> i32 {
    20
}

fn default_inner_padding_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn default_border_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_socket() -> PathBuf {
    PathBuf::from("/tmp/mononocle.sock")
}

fn default_desktops() -> u32 {
    4
}

fn default_mouse_jitter_threshold() -> f64 {
    5.0
}

fn default_fullscreen_holds_idle() -> bool {
    true
}

fn default_lock_bg_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn default_lock_fg_color() -> [f32; 4] {
    [0.3, 0.3, 0.3, 1.0]
}

fn default_lock_fg_active_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_unlock_socket() -> PathBuf {
    PathBuf::from("/tmp/mononocle-unlock.sock")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            background: None,
            background_align: default_background_align(),
            background_size: BackgroundSize::Cover,
            padding: default_padding(),
            corner_radius: 0.0,
            inner_padding: 0,
            inner_padding_color: default_inner_padding_color(),
            border_thickness: 0,
            border_color: default_border_color(),
            window_rules: Vec::new(),
            socket: default_socket(),
            desktops: default_desktops(),
            outputs: Vec::new(),
            screen_blank_timeout_secs: None,
            display_off_timeout_secs: None,
            mouse_jitter_threshold: default_mouse_jitter_threshold(),
            cursor_hide_timeout_secs: None,
            fullscreen_holds_idle: default_fullscreen_holds_idle(),
            lock_timeout_secs: None,
            lock_bg_color: default_lock_bg_color(),
            lock_fg_color: default_lock_fg_color(),
            lock_fg_active_color: default_lock_fg_active_color(),
            unlock_socket: default_unlock_socket(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if let (Some(blank), Some(off)) = (self.screen_blank_timeout_secs, self.display_off_timeout_secs) {
            if off < blank {
                return Err(format!(
                    "display_off_timeout_secs ({off}) must be >= screen_blank_timeout_secs ({blank})"
                ));
            }
        }
        if let (Some(blank), Some(lock)) = (self.screen_blank_timeout_secs, self.lock_timeout_secs) {
            if lock < blank {
                return Err(format!(
                    "lock_timeout_secs ({lock}) must be >= screen_blank_timeout_secs ({blank})"
                ));
            }
        }
        Ok(())
    }
}
