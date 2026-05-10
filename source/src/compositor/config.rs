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
        }
    }
}
