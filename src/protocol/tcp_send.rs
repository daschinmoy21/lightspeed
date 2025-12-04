use std::sync::Arc;
use std::time::Duration;

use crate::transfer::metadata::{FileMetadata, CHUNK_SIZE};
use anyhow::Result;
use async_channel;
use memmap2::Mmap;
use tokio::time::timeout;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const WORKERS: usize = 7;

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
        println!("[TCP] Parallel Sending to {}", self.addr);

        //mmap for cheap slicing
        let file = std::fs::File::open(&self.file_path)?;
        let mmap = Arc::new(unsafe { Mmap::map(&file)? });

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
        let (tx, rx) = async_channel::bounded(chunk_count);
        for id in 0..chunk_count {
            tx.send(id).await?;
        }
        drop(tx); //close queue

        //spawn workers
        let mut handlers = vec![];
        for _ in 0..WORKERS {
            let addr = self.addr.clone();
            let mmap_ref = mmap.clone();
            let mut rx = rx.clone();

            let handle = tokio::spawn(async move {
                while let Ok(id) = rx.recv().await {
                    let start = id * CHUNK_SIZE;
                    let end = ((id + 1) * CHUNK_SIZE).min(mmap_ref.len());
                    let chunk = &mmap_ref[start..end];
                    let size = chunk.len() as u32;

                    loop {
                        match Self::send_one_chunk(&addr, id as u32, chunk).await {
                            Ok(_) => break,
                            Err(e) => {
                                eprintln!("[TCP] Retry chunks {}:{}", id, e);
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        }
                    }
                }
            });
            handlers.push(handle);
        }
        for h in handlers {
            h.await?;
        }
        println!("[TCP] All chunks sent");
        Ok(meta.size)
    }

    async fn send_one_chunk(addr: &str, id: u32, chunk: &[u8]) -> Result<()> {
        let mut conn = timeout(Duration::from_secs(3), TcpStream::connect(addr)).await??;
        conn.write_all(&id.to_le_bytes()).await?;
        conn.write_all(&(chunk.len() as u32).to_le_bytes()).await?;
        conn.write_all(chunk).await?;

        let mut ack = [0u8; 1];
        conn.read_exact(&mut ack).await?;

        if ack[0] != 1 {
            anyhow::bail!("Invalid ACK");
        }

        println!("[TCP] Worker Sent Chunk {}", id);
        Ok(())
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
