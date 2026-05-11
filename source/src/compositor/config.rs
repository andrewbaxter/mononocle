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

/// A window rule that overrides decoration parameters for matching windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowRule {
    /// Regex matched against the window title.
    pub title: Option<String>,
    /// Regex matched against the app_id.
    pub app_id: Option<String>,
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
    /// Window rules applied in order; all matching rules are applied.
    #[serde(default)]
    pub window_rules: Vec<WindowRule>,
    /// Path for the IPC Unix socket.
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
    /// Number of virtual desktops.
    #[serde(default = "default_desktops")]
    pub desktops: u32,
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
            screen_blank_timeout_secs: None,
            display_off_timeout_secs: None,
            mouse_jitter_threshold: default_mouse_jitter_threshold(),
            cursor_hide_timeout_secs: None,
            fullscreen_holds_idle: default_fullscreen_holds_idle(),
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
        Ok(())
    }
}
