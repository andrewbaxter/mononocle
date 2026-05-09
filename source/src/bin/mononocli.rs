use {
    aargvark::{
        Aargvark,
        vark,
    },
    mononocle::ipc::{
        KillWindowArgs,
        ListWindows,
        Watch,
        protocol,
    },
    std::path::PathBuf,
};

#[derive(Aargvark)]
struct Args {
    /// IPC socket path.
    #[vark(flag = "--socket", flag = "-s")]
    socket: Option<PathBuf>,
    command: Command,
}

#[derive(Aargvark)]
enum Command {
    /// List all windows and their properties.
    ListWindows,
    /// Listen for window events and print them as they arrive.
    Listen,
    /// Show desktop by number (0-indexed).
    ShowDesktop(u32),
    /// Show window by id.
    ShowWindow(u64),
    /// Kill a window. Kills focused window if no id given.
    Kill(KillArgs),
}

#[derive(Aargvark)]
struct KillArgs {
    /// Window id to kill. Kills the focused window if not specified.
    id: Option<u64>,
}

fn default_socket() -> PathBuf {
    PathBuf::from("/tmp/mononocle.sock")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Args = vark();
    let socket = args.socket.unwrap_or_else(default_socket);
    let result = run(socket, args.command).await;
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run(socket: PathBuf, command: Command) -> Result<(), String> {
    match command {
        Command::ListWindows => {
            let mut client = protocol::Client::new(&socket).await?;
            let windows = client.send_req(ListWindows).await?;
            if windows.is_empty() {
                println!("No windows.");
            } else {
                for w in windows {
                    let visible = if w.is_visible {
                        " [visible]"
                    } else {
                        ""
                    };
                    let title = w.title.as_deref().unwrap_or("<no title>");
                    let app_id = w.app_id.as_deref().unwrap_or("<no app-id>");
                    println!("id={} desktop={} app_id={} title={}{visible}", w.id, w.desktop, app_id, title);
                }
            }
        },
        Command::Listen => {
            let mut client = protocol::Client::new(&socket).await?;
            loop {
                let events = client.send_req(Watch).await?;
                for event in events {
                    let json = serde_json::to_string(&event).unwrap_or_else(|_| "<serialize error>".into());
                    println!("{json}");
                }
            }
        },
        Command::ShowDesktop(n) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(n).await?;
            println!("Switched to desktop {n}.");
        },
        Command::ShowWindow(id) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(id).await?;
            println!("Showed window {id}.");
        },
        Command::Kill(args) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(KillWindowArgs { id: args.id }).await?;
            match args.id {
                Some(id) => println!("Sent kill to window {id}."),
                None => println!("Sent kill to focused window."),
            }
        },
    }
    Ok(())
}
