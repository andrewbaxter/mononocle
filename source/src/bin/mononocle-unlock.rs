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

    // Read the password hash while still root.
    let enc_pw = read_shadow_hash();
    tracing::info!("Read password hash, dropping privileges");

    // Drop root privileges.
    drop_privileges();
    tracing::info!("Privileges dropped, starting IPC server");

    let socket_path = std::env::var("MONONOCLE_UNLOCK_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run_server(socket_path, enc_pw));
}

fn read_shadow_hash() -> String {
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            eprintln!("getpwuid failed");
            std::process::exit(1);
        }

        let pw_passwd = std::ffi::CStr::from_ptr((*pw).pw_passwd).to_string_lossy().into_owned();
        let pw_name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();

        let enc = if pw_passwd == "x" {
            // Password is in shadow file.
            let name_c = std::ffi::CString::new(pw_name.as_str()).expect("CString");
            let sp = libc::getspnam(name_c.as_ptr());
            if sp.is_null() {
                eprintln!("getspnam failed for user {pw_name} — is this process running as root?");
                std::process::exit(1);
            }
            std::ffi::CStr::from_ptr((*sp).sp_pwdp).to_string_lossy().into_owned()
        } else {
            pw_passwd
        };

        tracing::debug!("Prepared to authorize user {pw_name}");
        enc
    }
}

fn drop_privileges() {
    unsafe {
        let gid = libc::getgid();
        let uid = libc::getuid();

        if libc::setgid(gid) != 0 {
            eprintln!("Unable to drop root (setgid)");
            std::process::exit(1);
        }
        if libc::setuid(uid) != 0 {
            eprintln!("Unable to drop root (setuid)");
            std::process::exit(1);
        }
        // Verify we can't restore root.
        if libc::setuid(0) != -1 || libc::setgid(0) != -1 {
            eprintln!("Unable to drop root (could restore)");
            std::process::exit(1);
        }
    }
}

async fn run_server(socket_path: PathBuf, enc_pw: String) {
    // Remove stale socket if it exists.
    let _ = std::fs::remove_file(&socket_path);

    let mut server = match unlock_protocol::Server::new(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unlock IPC server error: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("Unlock IPC socket at {}", socket_path.display());

    loop {
        let conn = match server.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Unlock IPC accept error: {e}");
                continue;
            }
        };
        let enc_pw = enc_pw.clone();
        tokio::spawn(async move {
            handle_connection(conn, enc_pw).await;
        });
    }
}

async fn handle_connection(mut conn: unlock_protocol::ServerConn, enc_pw: String) {
    loop {
        let req = match conn.recv_req().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("Unlock IPC recv error: {e}");
                break;
            }
        };
        let resp = match req {
            unlock_protocol::ServerReq::CheckPassword(respond, check) => {
                let success = verify_password(&check.password, &enc_pw);
                if !success {
                    tracing::debug!("Password verification failed");
                    // Brute-force delay.
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                respond(success)
            }
        };
        if let Err(e) = conn.send_resp(resp).await {
            tracing::debug!("Unlock IPC send error: {e}");
            break;
        }
    }
}

#[link(name = "crypt")]
unsafe extern "C" {
    fn crypt(key: *const libc::c_char, salt: *const libc::c_char) -> *mut libc::c_char;
}

fn verify_password(password: &str, enc_pw: &str) -> bool {
    let password_c = match std::ffi::CString::new(password) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let enc_c = match std::ffi::CString::new(enc_pw) {
        Ok(c) => c,
        Err(_) => return false,
    };
    unsafe {
        let result = crypt(password_c.as_ptr(), enc_c.as_ptr());
        if result.is_null() {
            return false;
        }
        let result_str = std::ffi::CStr::from_ptr(result);
        result_str.to_bytes() == enc_pw.as_bytes()
    }
}
