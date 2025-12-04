use std::sync::Arc;
use std::time::Duration;

use crate::transfer::metadata::{FileMetadata, CHUNK_SIZE};
use anyhow::Result;
use async_channel;
use memmap2::MmapOptions;
use tokio::time::timeout;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const WORKERS: usize = 6;

pub struct TcpSender {
    pub addr: String,
    pub file_path: String,
}

impl TcpSender {
    pub fn new(addr: String, file_path: String) -> Self {
        Self { addr, file_path }
    }

    // pub async fn send(&self) -> Result<u64> {
    //     println!(" [TCP] Connecting to {}", self.addr);
    //     let mut stream = TcpStream::connect(&self.addr).await?;
    //
    //     let meta = FileMetadata::from_file(&self.file_path)?;
    //     println!("[TCP] TCP Metadata {:?} ", meta);
    //
    //     let mut file = File::open(&self.file_path).await?;
    //     let mut buffer = vec![0u8; CHUNK_SIZE];
    //     let mut total_sent = 0u64;
    //     let mut chunk_id = 0u64;
    //
    //     loop {
    //         let bytes_read = file.read(&mut buffer).await?;
    //         if bytes_read == 0 {
    //             break;
    //         }
    //         let chunk_size = bytes_read as u32;
    //
    //         stream.write_all(&chunk_id.to_le_bytes()).await?;
    //         stream.write_all(&chunk_size.to_le_bytes()).await?;
    //         stream.write_all(&buffer[..bytes_read]).await?;
    //
    //         total_sent += bytes_read as u64;
    //         chunk_id += 1;
    //         println!("[TCP] Sent chunk {}", chunk_id);
    //     }
    //
    //     println!("[TCP] Sent file: {} bytes", total_sent);
    //
    //     Ok(total_sent)
    // }

    pub async fn parallel_send(&self) -> Result<u64> {
        println!("[TCP] Preparing file...");
        let file = std::fs::File::open(&self.file_path)?;
        let mmap = Arc::new(unsafe { MmapOptions::new().map(&file)? });

        let meta = FileMetadata::from_file(&self.file_path)?;
        println!("[TCP] Metadata:{:?}", meta);
        let chunk_count = meta.chunk_count as usize;

        // Send metadata first
        let mut meta_conn = TcpStream::connect(&self.addr).await?;
        let meta_bytes = meta.to_bytes()?;
        meta_conn
            .write_all(&(meta_bytes.len() as u32).to_le_bytes())
            .await?;
        meta_conn.write_all(&meta_bytes).await?;
        drop(meta_conn);

        //create job queue
        let (tx, rx) = async_channel::bounded(chunk_count as usize);

        //spawn workers asynchronously (lazy connection)
        let mut handlers = vec![];
        for _ in 0..WORKERS {
            let addr = self.addr.clone();
            let mmap_ref = mmap.clone();
            let rx = rx.clone();
            let meta = meta.clone();

            let handle = tokio::spawn(async move {
                let mut conn: Option<TcpStream> = None;

                while let Ok(id) = rx.recv().await {
                    // Lazy connection: connect only on first chunk
                    if conn.is_none() {
                        conn = match timeout(Duration::from_secs(3), TcpStream::connect(&addr)).await {
                            Ok(Ok(c)) => {
                                println!("[TCP] Worker connected to {}", addr);
                                Some(c)
                            }
                            _ => {
                                eprintln!("[TCP] Worker failed to connect to {}", addr);
                                return;
                            }
                        };
                    }
                    let conn = conn.as_mut().unwrap();

                    let start = id as u64 * CHUNK_SIZE;
                    let end = ((id as u64 + 1) * CHUNK_SIZE).min(meta.size);
                    let chunk = &mmap_ref[start as usize..end as usize];

                    // Send chunk header
                    if conn.write_all(&(id as u32).to_le_bytes()).await.is_err() { return; }
                    if conn.write_all(&(chunk.len() as u32).to_le_bytes()).await.is_err() { return; }

                    println!("[TCP] Sending chunk {} size {}", id, chunk.len());

                    // Send chunk data
                    if conn.write_all(chunk).await.is_err() { return; }

                    // Read ack
                    let mut ack = [0u8; 1];
                    if conn.read_exact(&mut ack).await.is_err() {
                        eprintln!("[TCP] Failed to read ack for chunk {}", id);
                        return;
                    }
                    if ack[0] != 1 {
                        eprintln!("[TCP] Invalid ack for chunk {}", id);
                        return;
                    }

                    println!("[TCP] Worker sent chunk {}", id);
                }
            });
            handlers.push(handle);
        }

        println!("[TCP] Starting transfer...");

        // Send chunks
        for id in 0..chunk_count {
            tx.send(id).await?;
        }
        drop(tx); //close queue

        for h in handlers {
            h.await?;
        }
        println!("[TCP] All chunks sent");
        Ok(meta.size)
    }
}

// let chunk_count = meta.chunk_count as usize;
//
// for chunk_id in 0..chunk_count {
//     let addr = self.addr.clone();
//     let start = chunk_id * CHUNK_SIZE;
//     let end = std::cmp::min(start + CHUNK_SIZE, mmap.len());
//     let chunk = mmap[start..end].to_vec();
//
//     let size = meta.size;
//     tokio::spawn(async move {
//         let mut conn = TcpStream::connect(&addr).await.unwrap();
//
//         conn.write_all(&(chunk_count as u32).to_le_bytes())
//             .await
//             .unwrap();
//         conn.write_all(&(size as u32).to_le_bytes()).await.unwrap(); // Send file size
//         conn.write_all(&(chunk_id as u32).to_le_bytes())
//             .await
//             .unwrap();
//         conn.write_all(&(chunk.len() as u32).to_le_bytes())
//             .await
//             .unwrap();
//         conn.write_all(&chunk).await.unwrap();
//
//         println!("[TCP] Sent chunk {}", chunk_id);
//     });
// }
// Ok(meta.size)
//     }
// }
