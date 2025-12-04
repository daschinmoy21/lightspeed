use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
use tokio::{net::UdpSocket, time};

pub const DISCOVERY_ADDR: &str = "239.255.0.1:9999";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DiscoveryPacket {
    pub hostname: String,
    pub tcp_port: u16,
    pub quic_port: u16,
}

// --------------------------------------------------------
// Broadcaster: sends "I exist" every 1 second
// --------------------------------------------------------
pub async fn start_broadcast(tcp_port: u16, quic_port: u16) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0").await?;

    let addr: SocketAddrV4 = DISCOVERY_ADDR.parse::<SocketAddrV4>().unwrap();

    let packet = DiscoveryPacket {
        hostname: gethostname::gethostname().to_string_lossy().to_string(),
        tcp_port,
        quic_port,
    };

    sock.set_multicast_ttl_v4(1)?;

    let bytes = serde_json::to_vec(&packet)?;

    loop {
        sock.send_to(&bytes, addr).await?;
        time::sleep(Duration::from_secs(1)).await;
    }
}

// --------------------------------------------------------
// Listener: prints peers it discovers
// --------------------------------------------------------
pub async fn start_listener() -> Result<()> {
    let socket = UdpSocket::bind(DISCOVERY_ADDR).await?;

    // Join multicast group
    socket.join_multicast_v4(Ipv4Addr::new(239, 255, 0, 1), Ipv4Addr::new(0, 0, 0, 0))?;

    let mut buf = vec![0u8; 1024];

    println!("Listening for peers on {DISCOVERY_ADDR}...");

    loop {
        let (len, addr) = socket.recv_from(&mut buf).await?;

        if let Ok(packet) = serde_json::from_slice::<DiscoveryPacket>(&buf[..len]) {
            println!("Peer discovered: {addr} -> {packet:?}");
        }
    }
}
