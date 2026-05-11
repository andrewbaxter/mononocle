use {
    mononocle::ipc::unlock_protocol,
    std::path::PathBuf,
};

const DEFAULT_SOCKET: &str = "/tmp/mononocle-unlock.sock";

fn main() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    let expected_password = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: mononocle-unlock-test <password>");
        std::process::exit(1);
    });

    let socket_path = std::env::var("MONONOCLE_UNLOCK_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run_server(socket_path, expected_password));
}

async fn run_server(socket_path: PathBuf, expected_password: String) {
    let _ = std::fs::remove_file(&socket_path);

    let mut server = match unlock_protocol::Server::new(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unlock test IPC server error: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("Unlock test IPC socket at {}", socket_path.display());

    loop {
        let conn = match server.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Unlock test IPC accept error: {e}");
                continue;
            }
        };
        let expected = expected_password.clone();
        tokio::spawn(async move {
            handle_connection(conn, expected).await;
        });
    }
}

async fn handle_connection(mut conn: unlock_protocol::ServerConn, expected: String) {
    loop {
        let req = match conn.recv_req().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("Unlock test IPC recv error: {e}");
                break;
            }
        };
        let resp = match req {
            unlock_protocol::ServerReq::CheckPassword(respond, check) => {
                let success = check.password == expected;
                if success {
                    tracing::info!("Password matched");
                } else {
                    tracing::debug!("Password did not match");
                }
                respond(success)
            }
        };
        if let Err(e) = conn.send_resp(resp).await {
            tracing::debug!("Unlock test IPC send error: {e}");
            break;
        }
    }
}
