use {
    serde::{
        Deserialize,
        Serialize,
    },
    std::path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to background image (PNG or JPEG).
    pub background: Option<PathBuf>,
    /// Padding around windows in logical pixels.
    #[serde(default = "default_padding")]
    pub padding: i32,
    /// Path for the IPC Unix socket.
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
    /// Number of virtual desktops.
    #[serde(default = "default_desktops")]
    pub desktops: u32,
}

fn default_padding() -> i32 {
    20
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
            padding: default_padding(),
            socket: default_socket(),
            desktops: default_desktops(),
        }
    }
}
