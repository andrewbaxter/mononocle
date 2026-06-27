use {
    schemars::JsonSchema,
    serde::{
        Deserialize,
        Serialize,
    },
};

glove::reqresp!(pub protocol {
    ListWindows(ListWindows) => ListWindowsResponse,
    ShowDesktop(ShowDesktopArgs) =>(),
    ShowWindow(u64) =>(),
    KillWindow(KillWindowArgs) =>(),
    ToggleFullscreen(ToggleFullscreenArgs) =>(),
    SetDesktop(SetDesktopArgs) =>(),
    Watch(Watch) => Vec < WindowEvent >,
});

glove::reqresp!(pub unlock_protocol {
    CheckPassword(CheckPassword) => bool,
});

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckPassword {
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KillWindowArgs {
    pub id: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListWindows;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListWindowsResponse {
    pub lock_inhibited: bool,
    pub windows: Vec<WindowInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDesktopArgs {
    pub desktop: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowDesktopArgs {
    pub desktop: u32,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleFullscreenArgs {
    pub id: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Watch;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum WindowEvent {
    LockInhibitedChanged {
        lock_inhibited: bool,
    },
    ShownDesktopChanged {
        desktop: u32,
    },
    ShownWindowChanged {
        window_id: Option<u64>,
    },
    WindowCreated {
        window: WindowInfo,
    },
    WindowDeleted {
        id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowInfo {
    pub app_id: Option<String>,
    pub desktop: u32,
    pub id: u64,
    pub is_visible: bool,
    pub title: Option<String>,
}
