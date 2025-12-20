use crate::transfer::metadata::{FileMetadata, CHUNK_SIZE};
use anyhow::Result;
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

        // Open the file for writing
        // LEARN: We use OpenOptions to create/overwrite.
        // `set_len` pre-allocates space on disk, which helps reduce fragmentation and ensures we have enough space.
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output_file)
            .await?;
        out.set_len(meta.size as u64).await?;
        
        // CRITICAL PERFORMANCE NOTE:
        // We are wrapping the file in an Async Mutex.
        // This means only ONE worker thread can write to the file at a time.
        // Even though we receive chunks in parallel, we serialize the writing.
        // Better approach: `File` on Linux behaves well with parallel `pwrite` (or `write_at`),
        // but `tokio::fs::File` requires mutable access for `seek+write`.
        // A `std::fs::File` with `spawn_blocking` or using `positioned-io` crates might be faster.
        let out = Arc::new(Mutex::new(out));

        loop {
            let (mut socket, peer_addr) = listener.accept().await?;
            println!("[TCP] Connection on {}", peer_addr);

            let out_clone = out.clone();
            let done_clone = done.clone();
            let expected_clone = expected_chunks.clone();
            let size_clone = file_size.clone();

            tokio::spawn(async move {
                loop {
                    let mut id_buf = [0u8; 4];
                    if socket.read_exact(&mut id_buf).await.is_err() {
                        // Connection closed or error
                        break;
                    }
                    let chunk_id = u32::from_le_bytes(id_buf) as u64;

                    let mut size_buf = [0u8; 4];
                    if socket.read_exact(&mut size_buf).await.is_err() {
                        eprintln!("[TCP] Failed to read chunk size for {}", chunk_id);
                        break;
                    }
                    let chunk_size = u32::from_le_bytes(size_buf) as u64;
                    
                    // Read Hash
                    let mut hash_buf = [0u8; 32];
                    if socket.read_exact(&mut hash_buf).await.is_err() {
                        eprintln!("[TCP] Failed to read chunk hash for {}", chunk_id);
                        break;
                    }

                    let chunk_size_usize = chunk_size as usize;
                    let mut chunk_data = vec![0u8; chunk_size_usize];
                    if socket.read_exact(&mut chunk_data).await.is_err() {
                        eprintln!("[TCP] Failed to read chunk data for {}", chunk_id);
                        break;
                    }

                    // VERIFY HASH
                    let calculated_hash = blake3::hash(&chunk_data);
                    if calculated_hash.as_bytes() != &hash_buf {
                        eprintln!("[TCP] Hash mismatch for chunk {}! Data corrupted.", chunk_id);
                        // In a real protocol, we would send a specific NACK code here.
                        // For now, we break/close connection which will trigger retry if implemented.
                        break;
                    }

                    println!("[TCP] Received chunk {} size {}", chunk_id, chunk_size);

                    let offset = chunk_id * CHUNK_SIZE as u64;

                    let mut file = out_clone.lock().await;
                    if let Err(e) = file.seek(std::io::SeekFrom::Start(offset)).await {
                        eprintln!("[TCP] Error seeking to chunk {chunk_id}: {e:?}");
                        break;
                    }
                    if let Err(e) = file.write_all(&chunk_data).await {
                        eprintln!("[TCP] Error writing chunk {chunk_id}: {e:?}");
                        break;
                    }
                    println!("[TCP] Wrote chunk {}", chunk_id);
                    done_clone.fetch_add(1, Ordering::SeqCst);
                    if let Err(e) = socket.write_all(&[1]).await {
                        eprintln!("[TCP] Error sending ack for chunk {chunk_id}: {e:?}");
                        break;
                    }
                }
            });

            // Check if done
            // LEARN: We use atomic loads to check shared state across threads without locking.
            let current_done = done.load(Ordering::SeqCst);
            let current_expected = expected_chunks.load(Ordering::SeqCst);
            if current_done == current_expected && current_expected > 0 {
                println!("[TCP] All chunks received ({} / {})", current_done, current_expected);

                println!("[TCP] All chunks received ({} / {})", current_done, current_expected);

                // NOTE: Global file integrity check is removed in favor of per-chunk BLAKE3 verification.
                println!("[TCP] File received successfully.");
                
                break;
            }
        }
        Ok(())
    }
}
