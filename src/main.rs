mod cli;
mod discovery;
mod protocol;
mod transfer;
mod ui; // Add UI module
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use clap::Parser;

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    format!("{:.2} {}", size, UNITS[unit_index])
}
use cli::{Cli, Commands};
#[tokio::main]
async fn main() -> anyhow::Result<()> {

    // Check if args are provided, else launch Full TUI
    if std::env::args().len() > 1 {
        // CLI Mode (legacy support)
        let cli = Cli::parse();
        match cli.command {
            Commands::Send { file: _, protocol: _ } => {
                // ... (Existing CLI logic could go here, but user wants TUI primarily)
                // For now, let's redirect to TUI or keep legacy for scripts?
                // User said "no like make it a tui entirely".
                // Let's ignore args for now and force TUI provided in standard run
                // converting args to initial state if needed.
                // But for simplicity, let's just launch the TUI.
            }
            Commands::Receive { protocol: _ } => {}
        }
    }
    
    // Auto-launch TUI
    // 1. Setup Shared State
    let peers = Arc::new(Mutex::new(HashMap::new()));
    
    // 3. Start Transfer Listeners (Background) with Dynamic Ports
    let (tcp_listener, tcp_port) = {
        let mut port = 9001;
        let mut listener = None;
        for p in 9001..9020 {
            if let Ok(l) = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", p)).await {
                listener = Some(l);
                port = p;
                break;
            }
        }
        (listener.expect("No free ports 9001-9020"), port)
    };
    
    // Send updated port to UI/State?
    // Ideally we'd display "Listening on :900X" in the UI.

    tokio::spawn(async move {
        let server = protocol::tcp::TcpProtocol::new(tcp_port);
        if let Err(_e) = server.run_server(tcp_listener).await {
            // eprintln!("TCP Server Error: {}", e);
        }
    });

    // QUIC Receiver (Try to match TCP port or +2)
    // For simplicity, let's try to bind QUIC to the SAME port (UDP)
    let quic_port = tcp_port;
    tokio::spawn(async move {
        if let Err(_e) = protocol::quic::QuicProtocol::start_server(quic_port).await {
             // eprintln!("QUIC Server Error: {}", e);
        }
    });

    // 4. Start Discovery with ACTUAL port
    tokio::spawn(async move {
        let _ = discovery::start_broadcast(tcp_port, quic_port).await;
    });
    
    let peers_clone = peers.clone();
    tokio::spawn(async move {
        let _ = discovery::start_listener(peers_clone).await;
    });

    // 5. Run TUI
    let _chosen_peer_opt = ui::run_tui(peers).await?;
    
    Ok(())
}

