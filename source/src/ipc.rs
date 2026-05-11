use {
    schemars::JsonSchema,
    serde::{
        Deserialize,
        Serialize,
    },
};

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
    WindowCreated {
        window: WindowInfo,
    },
    WindowDeleted {
        id: u64,
    },
    ShownWindowChanged {
        window_id: Option<u64>,
    },
    ShownDesktopChanged {
        desktop: u32,
    },
    LockInhibitedChanged {
        lock_inhibited: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListWindowsResponse {
    pub windows: Vec<WindowInfo>,
    pub lock_inhibited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KillWindowArgs {
    /// Kill the window with this id, or the focused window if None.
    pub id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToggleFullscreenArgs {
    /// Toggle fullscreen for this window id, or the focused window if None.
    pub id: Option<u64>,
}

// Distinct unit newtypes so glove's ReqTrait impl doesn't conflict.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListWindows;
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Watch;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetDesktopArgs {
    /// Desktop number to associate with the caller's PID tree.
    /// If None, uses the compositor's current desktop.
    pub desktop: Option<u32>,
}

glove::reqresp!(pub protocol {
    ListWindows(ListWindows) => ListWindowsResponse,
    ShowDesktop(u32) =>(),
    ShowWindow(u64) =>(),
    KillWindow(KillWindowArgs) =>(),
    ToggleFullscreen(ToggleFullscreenArgs) =>(),
    SetDesktop(SetDesktopArgs) =>(),
    Watch(Watch) => Vec < WindowEvent >,
});

// -- Unlock daemon IPC protocol --

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckPassword {
    pub password: String,
}

glove::reqresp!(pub unlock_protocol {
    CheckPassword(CheckPassword) => bool,
});
