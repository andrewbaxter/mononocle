use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use mononocle_ipc::{WindowEvent, WindowInfo, protocol};

/// State shared between the compositor thread and the IPC server thread.
pub struct SharedIpcState {
    pub windows: Vec<WindowInfo>,
    pub current_window_id: Option<u64>,
    pub current_desktop: u32,
    /// One queue per subscribed connection.
    pub event_queues: Vec<VecDeque<WindowEvent>>,
}

impl SharedIpcState {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            current_window_id: None,
            current_desktop: 0,
            event_queues: Vec::new(),
        }
    }
}

pub enum IpcCommand {
    ShowDesktop(u32),
    ShowWindow(u64),
    KillWindow(Option<u64>),
}

/// Spawns the IPC server in a dedicated thread with its own Tokio runtime.
pub fn spawn_ipc_server(
    socket_path: PathBuf,
    shared: Arc<Mutex<SharedIpcState>>,
    cmd_tx: std::sync::mpsc::Sender<IpcCommand>,
) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(run_server(socket_path, shared, cmd_tx));
    });
}

async fn run_server(
    socket_path: PathBuf,
    shared: Arc<Mutex<SharedIpcState>>,
    cmd_tx: std::sync::mpsc::Sender<IpcCommand>,
) {
    let mut server = match protocol::Server::new(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("IPC server error: {e}");
            return;
        }
    };
    tracing::info!("IPC socket at {}", socket_path.display());

    loop {
        let conn = match server.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("IPC accept error: {e}");
                continue;
            }
        };
        let shared = shared.clone();
        let cmd_tx = cmd_tx.clone();
        tokio::spawn(async move {
            handle_connection(conn, shared, cmd_tx).await;
        });
    }
}

async fn handle_connection(
    mut conn: protocol::ServerConn,
    shared: Arc<Mutex<SharedIpcState>>,
    cmd_tx: std::sync::mpsc::Sender<IpcCommand>,
) {
    let mut my_queue_idx: Option<usize> = None;

    loop {
        let req = match conn.recv_req().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("IPC recv error: {e}");
                break;
            }
        };

        let resp = match req {
            protocol::ServerReq::ListWindows(respond, _) => {
                let windows = shared.lock().unwrap().windows.clone();
                respond(windows)
            }

            protocol::ServerReq::ShowDesktop(respond, desktop) => {
                cmd_tx.send(IpcCommand::ShowDesktop(desktop)).ok();
                respond(())
            }

            protocol::ServerReq::ShowWindow(respond, id) => {
                cmd_tx.send(IpcCommand::ShowWindow(id)).ok();
                respond(())
            }

            protocol::ServerReq::KillWindow(respond, args) => {
                cmd_tx.send(IpcCommand::KillWindow(args.id)).ok();
                respond(())
            }

            protocol::ServerReq::Subscribe(respond, _) => {
                if my_queue_idx.is_none() {
                    let mut s = shared.lock().unwrap();
                    my_queue_idx = Some(s.event_queues.len());
                    s.event_queues.push(VecDeque::new());
                }
                respond(())
            }

            protocol::ServerReq::Poll(respond, _) => {
                let events = if let Some(idx) = my_queue_idx {
                    let mut s = shared.lock().unwrap();
                    if let Some(q) = s.event_queues.get_mut(idx) {
                        q.drain(..).collect::<Vec<_>>()
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };
                respond(events)
            }
        };

        if let Err(e) = conn.send_resp(resp).await {
            tracing::debug!("IPC send error: {e}");
            break;
        }
    }

    // Remove this connection's event queue to prevent stale queue growth.
    // Shift indices of later queues accordingly by removing from the vec.
    if let Some(idx) = my_queue_idx {
        let mut s = shared.lock().unwrap();
        if idx < s.event_queues.len() {
            s.event_queues.remove(idx);
        }
    }
}
