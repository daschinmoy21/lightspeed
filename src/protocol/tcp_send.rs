use std::sync::Arc;
use std::time::Duration;

use crate::transfer::metadata::FileMetadata;
use anyhow::Result;
use async_channel;
use memmap2::MmapOptions;
use tokio::time::timeout;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

// REMOVED: const WORKERS: usize = 7;

// Heuristic for concurrency optimization
fn optimize_concurrency(file_size: u64) -> usize {
    let num_cpus = num_cpus::get();
    
    if file_size < 100 * 1024 * 1024 {
        // < 100MB: Serial or low parallel to avoid connection overhead
        1.max(num_cpus / 4)
    } else if file_size < 1024 * 1024 * 1024 {
        // 100MB - 1GB: Moderate parallel
        4.max(num_cpus / 2)
    } else {
        // > 1GB: Max parallel (saturate bandwidth)
        8.max(num_cpus)
    }
}

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

    pub async fn parallel_send(&self, progress_tx: Option<tokio::sync::mpsc::Sender<crate::ui::ProgressEvent>>) -> Result<u64> {
        // println!("[TCP] Preparing file...");
        let file = std::fs::File::open(&self.file_path)?;
        // SAFETY: Mmap is unsafe...
        // ...
        let mmap = Arc::new(unsafe { MmapOptions::new().map(&file)? });

        let meta = FileMetadata::from_file(&self.file_path)?;
        println!("[TCP] Metadata:{:?}", meta);
        let chunk_count = meta.chunk_count as usize;

        // Send Started Event
        if let Some(tx) = &progress_tx {
            let _ = tx.send(crate::ui::ProgressEvent::Started {
                total_chunks: meta.chunk_count,
                total_size: meta.size,
                filename: meta.filename.clone(),
            }).await;
        }

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

        // Determine optimal worker count
        let worker_count = optimize_concurrency(meta.size);
        println!("[TCP] Optimizing: Using {} workers for {} file", worker_count, crate::format_bytes(meta.size));

        //spawn workers asynchronously (lazy connection)
        let mut join_set = tokio::task::JoinSet::new();
        
        for _ in 0..worker_count {
            let addr = self.addr.clone();
            let mmap_ref = mmap.clone();
            let rx = rx.clone();
            let meta = meta.clone();
            let progress_tx_clone = progress_tx.clone();

            join_set.spawn(async move {
                let mut conn: Option<TcpStream> = None;

                while let Ok(id) = rx.recv().await {
                    // Lazy connection: connect only on first chunk
                    if conn.is_none() {
                        conn = Some(
                            timeout(Duration::from_secs(5), TcpStream::connect(&addr))
                                .await
                                .map_err(|_| anyhow::anyhow!("Connection timeout to {}", addr))?
                                .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", addr, e))?
                        );
                        println!("[TCP] Worker connected to {}", addr);
                    }
                    let conn = conn.as_mut().unwrap();

                    let start = id as u64 * meta.chunk_size;
                    let end = ((id as u64 + 1) * meta.chunk_size).min(meta.size);
                    let chunk = &mmap_ref[start as usize..end as usize];
                    
                    // Compute BLAKE3 hash of the chunk
                    let hash = blake3::hash(chunk);

                    let mut retries = 0;
                    loop {
                        // Send chunk header
                        conn.write_all(&(id as u32).to_le_bytes()).await?;
                        conn.write_all(&(chunk.len() as u32).to_le_bytes()).await?;
                        conn.write_all(hash.as_bytes()).await?;

                        // Send chunk data
                        conn.write_all(chunk).await?;

                        // Read ack
                        let mut ack = [0u8; 1];
                        conn.read_exact(&mut ack).await?;
                        
                        if ack[0] == 1 {
                             // ACK
                             break;
                        } else {
                            // NACK
                            retries += 1;
                            if retries > 5 {
                                return Err(anyhow::anyhow!("Chunk {} failed after 5 retries. Network unsuitable?", id));
                            }
                            eprintln!("[TCP] Received NACK for chunk {}. Retrying ({}/5)...", id, retries);
                            if let Some(tx) = &progress_tx_clone {
                                let _ = tx.send(crate::ui::ProgressEvent::Error(format!("Chunk {} NACK retry {}/5", id, retries))).await;
                            }
                            tokio::time::sleep(Duration::from_millis(500)).await; // Backoff
                        }
                    }

                    // println!("[TCP] Worker sent chunk {}", id); // Remove spammy print
                    if let Some(tx) = &progress_tx_clone {
                        let _ = tx.send(crate::ui::ProgressEvent::ChunkSent { chunk_id: id, size: chunk.len() }).await;
                    }
                }
                Ok::<(), anyhow::Error>(())
            });
        }

        // println!("[TCP] Starting transfer...");

        // Send chunks to queue
        for id in 0..chunk_count {
            tx.send(id as u64).await?;
        }
        drop(tx); // close queue so workers break loop when empty

        // Wait for workers and check for errors
        while let Some(res) = join_set.join_next().await {
             match res {
                 Ok(Ok(())) => {}, // Worker finished successfully
                 Ok(Err(e)) => {
                     // A worker failed with an internal error (e.g. Broken Pipe)
                     return Err(anyhow::anyhow!("Worker failed: {}", e));
                 },
                 Err(e) => {
                     // A worker panicked or was cancelled
                     return Err(anyhow::anyhow!("Worker panicked: {}", e));
                 }
             }
        }

        if let Some(tx) = &progress_tx {
            let _ = tx.send(crate::ui::ProgressEvent::Done).await;
        }

        // println!("[TCP] All chunks sent");
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
