mod cli;
mod discovery;
mod protocol;
mod transfer;
use std::{
    collections::HashMap,
    io::Write,
    sync::{Arc, Mutex},
    time::Instant,
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
use cli::{Cli, Commands, Protocol};
use tokio::time::sleep;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Send { file, protocol } => {
            println!("Announcing presence via multicast");
            let peers = Arc::new(Mutex::new(HashMap::new()));

            tokio::spawn(async {
                let _ = discovery::start_broadcast(9001, 9001).await;
            });

            {
                let peers_clone = peers.clone();
                tokio::spawn(async move {
                    let _ = discovery::start_listener(peers_clone).await;
                });
            }
            println!("Discovering peers for 3 secs");
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let list = discovery::get_peers(&peers);

            if list.is_empty() {
                println!("No peers found");
                std::process::exit(1);
            }
            println!("Select a device to send to:\n ");
            for (i, p) in list.iter().enumerate() {
                println!("[{}] {} ({})", i, p.packet.hostname, p.addr.ip());
            }

            println!("Enter choice:");
            std::io::stdout().flush().unwrap();

            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            let idx: usize = input.trim().parse().unwrap();

            let chosen = &list[idx];
            let addr = format!("{}:{}", chosen.addr.ip(), chosen.packet.tcp_port);
            println!("Sending to {} ({})", chosen.packet.hostname, addr);

            match protocol {
                Protocol::Tcp => {
                    let start = Instant::now();
                    let sender = protocol::tcp_send::TcpSender::new(addr, file.clone());
                    let bytes_sent = sender.parallel_send().await?;
                    let time_taken = start.elapsed().as_secs_f64();
                    println!("Sent {} in {:.2} sec", format_bytes(bytes_sent), time_taken);
                }
                Protocol::Quic => {
                    println!("QUIC WORK IN PROGRESS");
                }
            }
            loop {
                sleep(std::time::Duration::from_secs(60)).await;
            }
        }
        Commands::Receive { protocol } => match protocol {
            Protocol::Tcp => {
                let server = protocol::tcp::TcpProtocol::new(9001);
                server.start_server().await
            }
            Protocol::Quic => {
                println!("QUIC WORK IN PROGRESS");
                Ok(())
            }
        },
    }
}

