use {
    crate::ipc::{
        ListWindowsResponse,
        ShowDesktopArgs,
        WindowEvent,
        WindowInfo,
        protocol,
    },
    std::{
        path::PathBuf,
        sync::{
            Arc,
            Mutex,
            mpsc::Sender,
        },
        thread::spawn as spawn_thread,
        time::Duration,
    },
    tokio::{
        runtime::Builder as RuntimeBuilder,
        spawn,
        sync::broadcast,
        time::timeout,
    },
};

async fn handle_connection(
    mut conn: protocol::ServerConn,
    shared: Arc<Mutex<SharedIpcState>>,
    cmd_tx: Sender<IpcCommand>,
    peer_pid: Option<u32>,
) {
    fn drain_receiver(rx: &mut broadcast::Receiver<WindowEvent>) -> Vec<WindowEvent> {
        let mut events = vec![];
        loop {
            match rx.try_recv() {
                Ok(e) => events.push(e),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!("Watch receiver lagged by {n} events");
                },
                Err(_) => break,
            }
        }
        events
    }

    let mut event_rx: Option<broadcast::Receiver<WindowEvent>> = None;
    loop {
        let req = match conn.recv_req().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("IPC recv error: {e}");
                break;
            },
        };
        let resp = match req {
            protocol::ServerReq::ListWindows(respond, _) => {
                let s = shared.lock().unwrap();
                respond(ListWindowsResponse {
                    windows: s.windows.clone(),
                    lock_inhibited: s.lock_inhibited,
                })
            },
            protocol::ServerReq::ShowDesktop(respond, ShowDesktopArgs { desktop, output }) => {
                cmd_tx.send(IpcCommand::ShowDesktop {
                    desktop,
                    output,
                }).ok();
                respond(())
            },
            protocol::ServerReq::ShowWindow(respond, id) => {
                cmd_tx.send(IpcCommand::ShowWindow(id)).ok();
                respond(())
            },
            protocol::ServerReq::KillWindow(respond, args) => {
                cmd_tx.send(IpcCommand::KillWindow(args.id)).ok();
                respond(())
            },
            protocol::ServerReq::ToggleFullscreen(respond, args) => {
                cmd_tx.send(IpcCommand::ToggleFullscreen(args.id)).ok();
                respond(())
            },
            protocol::ServerReq::SetDesktop(respond, args) => {
                if let Some(pid) = peer_pid {
                    cmd_tx.send(IpcCommand::SetDesktop {
                        pid,
                        desktop: args.desktop,
                    }).ok();
                }
                respond(())
            },
            protocol::ServerReq::Watch(respond, _) => {
                let events = if event_rx.is_none() {
                    let s = shared.lock().unwrap();
                    event_rx = Some(s.event_tx.subscribe());
                    let mut events: Vec<WindowEvent> =
                        s.windows.iter().map(|w| WindowEvent::WindowCreated { window: w.clone() }).collect();
                    events.push(WindowEvent::ShownDesktopChanged { desktop: s.current_desktop });
                    events.push(WindowEvent::ShownWindowChanged { window_id: s.current_window_id });
                    events.push(WindowEvent::LockInhibitedChanged { lock_inhibited: s.lock_inhibited });
                    events
                } else {
                    let rx = event_rx.as_mut().unwrap();
                    let mut events = drain_receiver(rx);
                    if events.is_empty() {
                        match timeout(Duration::from_secs(30), rx.recv()).await {
                            Ok(Ok(e)) => {
                                events.push(e);
                                events.extend(drain_receiver(rx));
                            },
                            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                                tracing::warn!("Watch receiver lagged by {n} events");
                                events.extend(drain_receiver(rx));
                            },
                            _ => { },
                        }
                    }
                    events
                };
                respond(events)
            },
        };
        if let Err(e) = conn.send_resp(resp).await {
            tracing::debug!("IPC send error: {e}");
            break;
        }
    }
}

pub enum IpcCommand {
    KillWindow(Option<u64>),
    SetDesktop {
        pid: u32,
        desktop: Option<u32>,
    },
    ShowDesktop {
        desktop: u32,
        output: Option<String>,
    },
    ShowWindow(u64),
    ToggleFullscreen(Option<u64>),
}

async fn run_server(socket_path: PathBuf, shared: Arc<Mutex<SharedIpcState>>, cmd_tx: Sender<IpcCommand>) {
    let mut server = match protocol::Server::new(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("IPC server error: {e}");
            return;
        },
    };
    tracing::info!("IPC socket at {}", socket_path.display());
    loop {
        let conn = match server.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("IPC accept error: {e}");
                continue;
            },
        };
        let peer_pid = conn.0.peer_cred().ok().map(|c| c.pid().unwrap_or(0) as u32);
        let shared = shared.clone();
        let cmd_tx = cmd_tx.clone();
        spawn(async move {
            handle_connection(conn, shared, cmd_tx, peer_pid).await;
        });
    }
}

pub struct SharedIpcState {
    pub current_desktop: u32,
    pub current_window_id: Option<u64>,
    pub event_tx: broadcast::Sender<WindowEvent>,
    pub lock_inhibited: bool,
    pub windows: Vec<WindowInfo>,
}

impl SharedIpcState {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(1000);
        Self {
            windows: Vec::new(),
            current_window_id: None,
            current_desktop: 0,
            lock_inhibited: false,
            event_tx,
        }
    }
}

pub fn spawn_ipc_server(socket_path: PathBuf, shared: Arc<Mutex<SharedIpcState>>, cmd_tx: Sender<IpcCommand>) {
    spawn_thread(move || {
        let rt = RuntimeBuilder::new_current_thread().enable_all().build().expect("tokio runtime");
        rt.block_on(run_server(socket_path, shared, cmd_tx));
    });
}
