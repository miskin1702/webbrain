use clap::{Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use webbrain_workspace::auth::AuthManager;
use webbrain_workspace::paths::PathSandbox;
use webbrain_workspace::server::{run_server, ServerState};
use webbrain_workspace::session::WorkspaceSession;
use webbrain_workspace::watcher::WorkspaceWatcher;

#[derive(Parser)]
#[command(name = "webbrain-workspace")]
#[command(author = "WebBrain Team")]
#[command(version)]
#[command(
    about = "WebBrain Local Workspace Bridge Daemon - Secure AI coding bridge for in-browser agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the local workspace bridge WebSocket server
    Serve {
        /// Path to the authorized workspace root directory
        #[arg(short, long)]
        root: PathBuf,

        /// Port to bind the loopback WebSocket server
        #[arg(short, long, default_value_t = 18374)]
        port: u16,

        /// Explicit pairing token (generated and saved if omitted)
        #[arg(short, long)]
        token: Option<String>,

        /// Allowed Chrome/Firefox extension ID (e.g. for strict origin checking)
        #[arg(long)]
        extension_id: Option<String>,

        /// Allow the AI agent to edit files via targeted patch
        #[arg(long, default_value_t = true)]
        allow_write: bool,

        /// Allow the AI agent to execute validation commands (tests, lint)
        #[arg(long, default_value_t = false)]
        allow_command: bool,

        /// Logging level (error, warn, info, debug)
        #[arg(long, default_value = "info")]
        log_level: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            root,
            port,
            token,
            extension_id,
            allow_write,
            allow_command,
            log_level: _,
        } => {
            // 1. Initialize and canonicalize path sandbox
            let sandbox = match PathSandbox::new(&root) {
                Ok(sb) => Arc::new(sb),
                Err(e) => {
                    eprintln!("[webbrain-workspace] ERROR: Invalid workspace root: {e}");
                    std::process::exit(1);
                }
            };

            // 2. Initialize AuthManager (token + origin verification)
            let auth = Arc::new(AuthManager::new(token, extension_id));

            // 3. Initialize WorkspaceSession
            let session = Arc::new(WorkspaceSession::new(
                sandbox.canonical_root().to_path_buf(),
                allow_write,
                allow_command,
            ));

            // 4. Setup event broadcast channel & start filesystem watcher
            let (event_tx, _) = broadcast::channel(128);
            let _watcher = match WorkspaceWatcher::new((*sandbox).clone(), event_tx.clone()) {
                Ok(w) => Some(w),
                Err(e) => {
                    eprintln!(
                        "[webbrain-workspace] WARNING: Could not start filesystem watcher: {e}"
                    );
                    None
                }
            };

            // 5. Bind loopback TCP listener
            let bind_addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
            let listener = match TcpListener::bind(bind_addr).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!(
                        "[webbrain-workspace] ERROR: Failed to bind to 127.0.0.1:{port}: {e}"
                    );
                    std::process::exit(1);
                }
            };

            let state = ServerState {
                sandbox: sandbox.clone(),
                auth: auth.clone(),
                session: session.clone(),
                event_tx,
            };

            let (shutdown_tx, shutdown_rx) = watch::channel(false);

            // Print safe startup banner
            println!("=======================================================");
            println!("  WebBrain Local Workspace Bridge Daemon (v1)");
            println!("=======================================================");
            println!("  WebSocket URL:   ws://127.0.0.1:{port}");
            println!("  Authorized Root: {}", sandbox.canonical_root().display());
            println!("  Root Name:       {}", session.root_name);
            println!(
                "  Capabilities:    Read: true | Write: {} | Command: {}",
                session.allow_write, session.allow_command
            );
            println!("  Pairing Token:   {}", auth.token());
            println!("-------------------------------------------------------");
            println!("  Status: Ready. Waiting for WebBrain connection.");
            println!("  Press Ctrl+C to terminate cleanly.");
            println!("=======================================================");

            // Spawn server task
            let server_handle = tokio::spawn(async move {
                if let Err(e) = run_server(listener, state, shutdown_rx).await {
                    eprintln!("[webbrain-workspace] Server error: {e}");
                }
            });

            // Wait for Ctrl+C
            tokio::signal::ctrl_c().await?;
            println!("\n[webbrain-workspace] Shutdown signal received. Closing bridge...");
            let _ = shutdown_tx.send(true);
            let _ = server_handle.await;
            println!("[webbrain-workspace] Daemon exited cleanly.");
        }
    }

    Ok(())
}
