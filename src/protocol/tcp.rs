use crate::transfer::metadata::{FileMetadata, CHUNK_SIZE};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Mutex;
use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::TcpListener,
};

pub struct TcpProtocol {
    pub port: u16,
}

impl TcpProtocol {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub async fn start_server(&self) -> Result<()> {
        let addr = format!("0.0.0.0:{}", self.port);
        let listener = TcpListener::bind(&addr).await?;
        println!("[TCP] Listening on {}", addr);

        // Receive metadata first
        let (mut meta_socket, _) = listener.accept().await?;
        let mut len_buf = [0u8; 4];
        meta_socket.read_exact(&mut len_buf).await?;
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut meta_bytes = vec![0u8; len];
        meta_socket.read_exact(&mut meta_bytes).await?;
        let meta: FileMetadata = FileMetadata::deserialize_metadata(&meta_bytes)?;
        let basename = Path::new(&meta.filename)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let output_file = format!("rec_{}", basename);
        println!("[TCP] Receiving file: {}", output_file);

        let done = Arc::new(AtomicUsize::new(0));
        let expected_chunks = Arc::new(AtomicUsize::new(meta.chunk_count as usize));
        let file_size = Arc::new(AtomicUsize::new(meta.size as usize));

        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_file)
            .await?;
        out.set_len(meta.size as u64).await?;
        let out = Arc::new(Mutex::new(out));

        loop {
            let (mut socket, peer_addr) = listener.accept().await?;
            println!("[TCP] Connection on {}", peer_addr);

            let out_clone = out.clone();
            let done_clone = done.clone();
            let expected_clone = expected_chunks.clone();
            let size_clone = file_size.clone();

            tokio::spawn(async move {
                let mut id_buf = [0u8; 4];
                if socket.read_exact(&mut id_buf).await.is_err() {
                    eprintln!("[TCP] Failed to read chunk id");
                    return;
                }
                let chunk_id = u32::from_le_bytes(id_buf) as u64;

                let mut size_buf = [0u8; 4];
                if socket.read_exact(&mut size_buf).await.is_err() {
                    eprintln!("[TCP] Failed to read chunk size for {}", chunk_id);
                    return;
                }
                let chunk_size = u32::from_le_bytes(size_buf) as u64;
                let chunk_size_usize = chunk_size as usize;
                let mut chunk_data = vec![0u8; chunk_size_usize];
                if socket.read_exact(&mut chunk_data).await.is_err() {
                    eprintln!("[TCP] Failed to read chunk data for {}", chunk_id);
                    return;
                }

                println!("[TCP] Received chunk {} size {}", chunk_id, chunk_size);

                let offset = chunk_id * CHUNK_SIZE as u64;

                let mut file = out_clone.lock().await;
                if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                    eprintln!("[TCP] Error seeking to chunk {chunk_id}: {e:?}");
                    return;
                }
                if let Err(e) = file.write_all(&chunk_data).await {
                    eprintln!("[TCP] Error writing chunk {chunk_id}: {e:?}");
                    return;
                }
                println!("[TCP] Wrote chunk {}", chunk_id);
                if let Err(e) = socket.write_all(&[1]).await {
                    eprintln!("[TCP] Error sending ack for chunk {chunk_id}: {e:?}");
                }
            });

            // Check if done
            if done.load(Ordering::SeqCst) == expected_chunks.load(Ordering::SeqCst)
                && expected_chunks.load(Ordering::SeqCst) > 0
            {
                println!("[TCP] All chunks received");

                // Verify file integrity
                match std::fs::File::open(&output_file) {
                    Ok(mut file) => {
                        let mut hasher = Sha256::new();
                        if std::io::copy(&mut file, &mut hasher).is_ok() {
                            let received_hash = format!("{:x}", hasher.finalize());
                            if received_hash == meta.hash {
                                println!("[TCP] File integrity verified successfully");
                            } else {
                                eprintln!("[TCP] Hash mismatch! Expected {}, got {}", meta.hash, received_hash);
                            }
                        } else {
                            eprintln!("[TCP] Failed to compute hash of received file");
                        }
                    }
                    Err(e) => eprintln!("[TCP] Failed to open received file for verification: {}", e),
                }

                break;
            }
        }
        Ok(())
    }
}
