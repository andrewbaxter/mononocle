use {
    libc::{
        c_char,
        getgid,
        getpwuid,
        getspnam,
        getuid,
        setgid,
        setuid,
    },
    mononocle::ipc::unlock_protocol,
    std::{
        env::var,
        ffi::{
            CStr,
            CString,
        },
        fs::remove_file,
        path::PathBuf,
        process::exit,
        time::Duration,
    },
    tokio::{
        runtime::Builder as RuntimeBuilder,
        spawn,
        time::sleep,
    },
};

const DEFAULT_SOCKET: &str = "/run/mononocle-unlock.sock";

#[link(name = "crypt")]
unsafe extern "C" {
    fn crypt(key: *const c_char, salt: *const c_char) -> *mut c_char;
}

async fn handle_connection(mut conn: unlock_protocol::ServerConn, enc_pw: String) {
    fn verify_password(password: &str, enc_pw: &str) -> bool {
        let password_c = match CString::new(password) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let enc_c = match CString::new(enc_pw) {
            Ok(c) => c,
            Err(_) => return false,
        };
        unsafe {
            let result = crypt(password_c.as_ptr(), enc_c.as_ptr());
            if result.is_null() {
                return false;
            }
            CStr::from_ptr(result).to_bytes() == enc_pw.as_bytes()
        }
    }

    loop {
        let req = match conn.recv_req().await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("Unlock IPC recv error: {e}");
                break;
            },
        };
        let resp = match req {
            unlock_protocol::ServerReq::CheckPassword(respond, check) => {
                let success = verify_password(&check.password, &enc_pw);
                if !success {
                    tracing::debug!("Password verification failed");
                    sleep(Duration::from_secs(2)).await;
                }
                respond(success)
            },
        };
        if let Err(e) = conn.send_resp(resp).await {
            tracing::debug!("Unlock IPC send error: {e}");
            break;
        }
    }
}

fn main() {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    fn read_shadow_hash() -> String {
        unsafe {
            let uid = getuid();
            let pw = getpwuid(uid);
            if pw.is_null() {
                eprintln!("getpwuid failed");
                exit(1);
            }
            let pw_passwd = CStr::from_ptr((*pw).pw_passwd).to_string_lossy().into_owned();
            let pw_name = CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
            let enc = if pw_passwd == "x" {
                let name_c = CString::new(pw_name.as_str()).expect("CString");
                let sp = getspnam(name_c.as_ptr());
                if sp.is_null() {
                    eprintln!("getspnam failed for user {pw_name} — is this process running as root?");
                    exit(1);
                }
                CStr::from_ptr((*sp).sp_pwdp).to_string_lossy().into_owned()
            } else {
                pw_passwd
            };
            tracing::debug!("Prepared to authorize user {pw_name}");
            enc
        }
    }

    fn drop_privileges() {
        unsafe {
            let gid = getgid();
            let uid = getuid();
            if setgid(gid) != 0 {
                eprintln!("Unable to drop root (setgid)");
                exit(1);
            }
            if setuid(uid) != 0 {
                eprintln!("Unable to drop root (setuid)");
                exit(1);
            }
            if setuid(0) != -1 || setgid(0) != -1 {
                eprintln!("Unable to drop root (could restore)");
                exit(1);
            }
        }
    }

    let enc_pw = read_shadow_hash();
    tracing::info!("Read password hash, dropping privileges");
    drop_privileges();
    tracing::info!("Privileges dropped, starting IPC server");
    RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(
            run_server(
                var("MONONOCLE_UNLOCK_SOCKET")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET)),
                enc_pw,
            ),
        );
}

async fn run_server(socket_path: PathBuf, enc_pw: String) {
    let _ = remove_file(&socket_path);
    let mut server = match unlock_protocol::Server::new(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unlock IPC server error: {e}");
            exit(1);
        },
    };
    tracing::info!("Unlock IPC socket at {}", socket_path.display());
    loop {
        let conn = match server.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Unlock IPC accept error: {e}");
                continue;
            },
        };
        let enc_pw = enc_pw.clone();
        spawn(async move {
            handle_connection(conn, enc_pw).await;
        });
    }
}
