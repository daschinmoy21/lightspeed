mod cli;
mod discovery;
mod protocol;
mod transfer;
use std::time::Instant;

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
            tokio::spawn(async {
                let _ = discovery::start_broadcast(9000, 9001).await;
            });
            match protocol {
                Protocol::Tcp => {
                    let start = Instant::now();
                    let sender =
                        protocol::tcp_send::TcpSender::new("127.0.0.1:9001".to_string(), file.clone());
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
                server.start_server().await?;
            }
            Protocol::Quic => {
                println!("QUIC WORK IN PROGRESS");
            }
        },
    }
    Ok(())
}
