use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WindowInfo {
    pub id: u64,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub desktop: u32,
    pub is_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WindowEvent {
    WindowCreated { window: WindowInfo },
    WindowDeleted { id: u64 },
    ShownWindowChanged { window_id: Option<u64> },
    ShownDesktopChanged { desktop: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KillWindowArgs {
    /// Kill the window with this id, or the focused window if None.
    pub id: Option<u64>,
}

// Distinct unit newtypes so glove's ReqTrait impl doesn't conflict.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListWindows;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Subscribe;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Poll;

glove::reqresp!(pub protocol {
    ListWindows(ListWindows) => Vec<WindowInfo>,
    ShowDesktop(u32) => (),
    ShowWindow(u64) => (),
    KillWindow(KillWindowArgs) => (),
    Subscribe(Subscribe) => (),
    Poll(Poll) => Vec<WindowEvent>,
});
