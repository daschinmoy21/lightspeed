use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
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

#[derive(Clone)]
pub struct DiscoveryPeer {
    pub addr: SocketAddr,
    pub packet: DiscoveryPacket,
}

// Broadcaster: sends "I exist" every 1 second
pub async fn start_broadcast(tcp_port: u16, quic_port: u16) -> Result<()> {
    let local_ip = match local_ip_address::local_ip()? {
        IpAddr::V4(v4) => v4,
        _ => anyhow::bail!("IPv6 not supported for broadcast"),
    };
    println!("Broadcasting from IP: {}", local_ip);

    let sock = UdpSocket::bind(format!("{}:0", local_ip)).await?;

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

// Listener: maintains a realtime list of discovered peers
pub async fn start_listener(peers: Arc<Mutex<HashMap<String, DiscoveryPeer>>>) -> Result<()> {
    //join multicast
    let local_ip = match local_ip_address::local_ip()? {
        IpAddr::V4(v4) => v4,
        _ => anyhow::bail!("IPV6 not supported for discovery"),
    };
    println!("Listening on IP: {}", local_ip);

    let socket = UdpSocket::bind(format!("{}:9999", local_ip)).await?;

    socket.join_multicast_v4(Ipv4Addr::new(239, 255, 0, 1), local_ip)?;

    let mut buf = vec![0u8;2048];

    loop{
        let (len, addr) = socket.recv_from(&mut buf ).await?;
        if let Ok(packet) = serde_json::from_slice::<DiscoveryPacket>(&buf[..len]) {
            match addr {
                SocketAddr::V4(v4_addr) => {
                    let ip = v4_addr.ip();
                    let key = format!("{}:{}", ip, packet.tcp_port);

                    let peer = DiscoveryPeer {
                        addr: SocketAddr::V4(SocketAddrV4::new(*ip, packet.tcp_port)),
                        packet: packet.clone(),
                    };
                    let mut map = peers.lock().unwrap();
                    let is_new = !map.contains_key(&key);
                    map.insert(key, peer);

                    if is_new {
                        println!("Peer Discovered: {} -> {:?}", addr, packet);
                    }
                }
                _ => {
                    // Ignore IPv6
                }
            }
        }
    }
}

pub fn get_peers(peers: &Arc<Mutex<HashMap<String, DiscoveryPeer>>>) -> Vec<DiscoveryPeer> {
    peers.lock().unwrap().values().cloned().collect()
}
