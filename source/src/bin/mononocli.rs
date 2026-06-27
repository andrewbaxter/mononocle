use {
    aargvark::{
        Aargvark,
        vark,
    },
    mononocle::ipc::{
        KillWindowArgs,
        ListWindows,
        SetDesktopArgs,
        ShowDesktopArgs,
        ToggleFullscreenArgs,
        Watch,
        protocol,
    },
    serde_json::to_string,
    std::{
        path::PathBuf,
        process::exit,
    },
};

#[derive(Aargvark)]
struct Args {
    command: Command,
    #[vark(flag = "--socket", flag = "-s")]
    socket: Option<PathBuf>,
}

#[derive(Aargvark)]
enum Command {
    Kill(KillArgs),
    Listen,
    ListWindows,
    SetDesktop(SetDesktopCliArgs),
    ShowDesktop(ShowDesktopCliArgs),
    ShowWindow(u64),
    ToggleFullscreen(ToggleFullscreenCliArgs),
}

#[derive(Aargvark)]
struct KillArgs {
    id: Option<u64>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Args = vark();
    if let Err(e) = run(args.socket.unwrap_or_else(|| PathBuf::from("/tmp/mononocle.sock")), args.command).await {
        eprintln!("Error: {e}");
        exit(1);
    }
}

async fn run(socket: PathBuf, command: Command) -> Result<(), String> {
    match command {
        Command::ListWindows => {
            let mut client = protocol::Client::new(&socket).await?;
            println!(
                "{}",
                to_string(
                    &client.send_req(ListWindows).await?,
                ).map_err(|e| format!("Failed to serialize response: {e}"))?
            );
        },
        Command::Listen => {
            let mut client = protocol::Client::new(&socket).await?;
            loop {
                let events = client.send_req(Watch).await?;
                for event in events {
                    println!("{}", to_string(&event).map_err(|e| format!("Failed to serialize event: {e}"))?);
                }
            }
        },
        Command::ShowDesktop(args) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(ShowDesktopArgs {
                desktop: args.desktop,
                output: args.output,
            }).await?;
        },
        Command::ShowWindow(id) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(id).await?;
        },
        Command::Kill(args) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(KillWindowArgs { id: args.id }).await?;
        },
        Command::ToggleFullscreen(args) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(ToggleFullscreenArgs { id: args.id }).await?;
        },
        Command::SetDesktop(args) => {
            let mut client = protocol::Client::new(&socket).await?;
            client.send_req(SetDesktopArgs { desktop: args.desktop }).await?;
        },
    }
    Ok(())
}

#[derive(Aargvark)]
struct SetDesktopCliArgs {
    desktop: Option<u32>,
}

#[derive(Aargvark)]
struct ShowDesktopCliArgs {
    desktop: u32,
    #[vark(flag = "--output", flag = "-o")]
    output: Option<String>,
}

#[derive(Aargvark)]
struct ToggleFullscreenCliArgs {
    id: Option<u64>,
}
