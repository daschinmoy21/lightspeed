use crate::transfer::metadata::FileMetadata;
use anyhow::Result;
use quinn::Endpoint;
use std::{net::SocketAddr, sync::Arc};
use tokio::io::AsyncWriteExt;

// --- QUIC Helper Configuration (Certificates) ---

/// Generates a self-signed certificate for the server
fn make_server_config() -> Result<(quinn::ServerConfig, Vec<u8>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert_der = cert.cert.der().to_vec();
    let priv_key = cert.signing_key.serialize_der();
    
    let priv_key = rustls::pki_types::PrivateKeyDer::Pkcs8(priv_key.into());
    let cert_chain = vec![rustls::pki_types::CertificateDer::from(cert_der.clone())];

    let mut server_config = quinn::ServerConfig::with_single_cert(cert_chain, priv_key)?;
    let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
    transport_config.max_concurrent_uni_streams(0_u8.into()); 
    
    Ok((server_config, cert_der))
}

/// Client config that trusts any certificate (TOFU/Insecure for local transfer)
fn make_client_config() -> quinn::ClientConfig {
    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    
    // Wrap the rustls config in Quinn's adapter
    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap();
    quinn::ClientConfig::new(Arc::new(quic_config))
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
         Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_SHA1_Legacy,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

// --- QUIC Protocol Implementation ---

pub struct QuicProtocol;

impl QuicProtocol {
    pub async fn start_server(port: u16) -> Result<()> {
        let (server_config, _) = make_server_config()?;
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let endpoint = Endpoint::server(server_config, addr)?;
        
        println!("[QUIC] Listening on {}", endpoint.local_addr()?);

        while let Some(conn) = endpoint.accept().await {
            let conn = conn.await?;
            println!("[QUIC] Connection accepted from {}", conn.remote_address());
            
            tokio::spawn(async move {
                // Main Connection Loop
                // For simplified "LightSpeed" over QUIC, we can just accept Uni streams.
                // The FIRST stream MUST be metadata.
                // Subsequent streams are chunks.
                
                // Wait for metadata stream
                let mut meta_stream = match conn.accept_uni().await {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("[QUIC] Connection failed to open metadata stream: {}", e);
                        return;
                    }
                };

                // Read Metadata size (u32)
                let mut len_buf = [0u8; 4];
                if let Err(e) = meta_stream.read_exact(&mut len_buf).await {
                    eprintln!("[QUIC] Failed to read metadata length: {}", e);
                    return;
                }
                let len = u32::from_le_bytes(len_buf) as usize;
                
                // Read Metadata JSON
                let mut meta_bytes = vec![0u8; len];
                if let Err(e) = meta_stream.read_exact(&mut meta_bytes).await {
                    eprintln!("[QUIC] Failed to read metadata body: {}", e);
                    return;
                }

                let meta: FileMetadata = match FileMetadata::deserialize_metadata(&meta_bytes) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("[QUIC] Failed to deserialize metadata: {}", e);
                        return;
                    }
                };
                
                let output_file = format!("quic_rec_{}", meta.filename); // Simple rename for test
                println!("[QUIC] Receiving file: {} ({} chunks)", output_file, meta.chunk_count);

                // Prepare Output File
                // Using std::fs::File with pre-allocation (similar to TCP)
                let file = match std::fs::File::create(&output_file) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("[QUIC] Failed to create output file: {}", e);
                        return;
                    }
                };
                if let Err(e) = file.set_len(meta.size) {
                    eprintln!("[QUIC] Failed to pre-allocate file: {}", e);
                    return;
                }
                
                // Use OS-level positioning (pwrite) via `std::os::unix::fs::FileExt` on Linux
                // This allows truly parallel writes without a Mutex lock!
                // Since user is on Linux, this is the optimal path.
                use std::os::unix::fs::FileExt;
                let file = Arc::new(file);

                let done_chunks = Arc::new(std::sync::atomic::AtomicU64::new(0));

                // Accept chunk streams in parallel
                loop {
                    match conn.accept_uni().await {
                        Ok(mut stream) => {
                            let file_ref = file.clone();
                            let done_ref = done_chunks.clone();
                            let total_chunks = meta.chunk_count;
                            let meta = meta.clone(); // Need meta for chunk_size inside task

                            tokio::spawn(async move {
                                // Read Chunk Header: [ID: u32][Size: u32][Hash: 32 bytes]
                                let mut header = [0u8; 40]; // 4 + 4 + 32
                                if stream.read_exact(&mut header).await.is_err() {
                                    return;
                                }

                                let chunk_id = u32::from_le_bytes(header[0..4].try_into().unwrap()) as u64;
                                let chunk_size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
                                let expected_hash = &header[8..40];

                                let mut data = vec![0u8; chunk_size];
                                if stream.read_exact(&mut data).await.is_err() {
                                    return;
                                }

                                // Verify Hash
                                let hash = blake3::hash(&data);
                                if hash.as_bytes() != expected_hash {
                                    eprintln!("[QUIC] Hash mismatch for chunk {}", chunk_id);
                                    return;
                                }

                                // Write to disk (Parallel safe PWRITE)
                                let offset = chunk_id * meta.chunk_size;
                                if let Err(e) = file_ref.write_all_at(&data, offset) {
                                    eprintln!("[QUIC] Disk write error chunk {}: {}", chunk_id, e);
                                    return;
                                }

                                let finished = done_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                                if finished % 100 == 0 || finished == total_chunks {
                                    println!("[QUIC] Progress: {}/{}", finished, total_chunks);
                                }
                            });
                        }
                        Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                            // Normal close
                            println!("[QUIC] Transfer complete (connection closed by sender).");
                            break;
                        }
                        Err(_e) => {
                            // eprintln!("[QUIC] Stream accept error: {}", e); 
                            break; 
                        }
                    }
                }
            });
        }
        Ok(())
    }

    pub async fn send_file(addr_str: String, file_path: String) -> Result<u64> {
        let addr: SocketAddr = addr_str.parse()?;
        let client_config = make_client_config();
        
        // Ensure endpoint binds to explicit V4 IP 0.0.0.0
        let mut endpoint = Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
        endpoint.set_default_client_config(client_config);

        println!("[QUIC] Connecting to {}...", addr);
        let connection = endpoint.connect(addr, "localhost")?.await?;
        println!("[QUIC] Connected!");

        // Read File & Meta
        let file = std::fs::File::open(&file_path)?;
        let meta = FileMetadata::from_file(&file_path)?;
        // Safety: Mmap
        let mmap = Arc::new(unsafe { memmap2::MmapOptions::new().map(&file)? });

        // 1. Send Metadata (First Uni Stream)
        let mut meta_stream = connection.open_uni().await?;
        let meta_bytes = meta.to_bytes()?;
        meta_stream.write_all(&(meta_bytes.len() as u32).to_le_bytes()).await?;
        meta_stream.write_all(&meta_bytes).await?;
        meta_stream.finish()?;


        // 2. Blast Chunks (Parallel Uni Streams)
        // QUIC handles congestion, so we can just spawn tasks to push streams.
        // However, spawning 10,000 tasks for a large file is bad memory-wise.
        // We should use a bounded semaphore or worker pool similar to TCP,
        // BUT here we don't manage connections, just streams on the SAME connection.
        
        let concurrency = 200; // Allow 200 concurrent streams. QUIC flow control will handle backpressure.
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let conn_arc = Arc::new(connection);

        let mut joins = vec![];

        for id in 0..meta.chunk_count {
            let permit = semaphore.clone().acquire_owned().await?;
            let conn = conn_arc.clone();
            let mmap = mmap.clone();
            
            joins.push(tokio::spawn(async move {
                let _permit = permit; // Hold permit until task done
                let chunk_id = id;
                
                let start = chunk_id * meta.chunk_size;
                let end = ((chunk_id + 1) * meta.chunk_size).min(meta.size);
                let chunk_data = &mmap[start as usize..end as usize];
                
                let hash = blake3::hash(chunk_data);

                // Open Stream
                match conn.open_uni().await {
                    Ok(mut stream) => {
                        // Header: [ID: u32][Size: u32][Hash: 32]
                        let mut header = Vec::with_capacity(40);
                        header.extend_from_slice(&(chunk_id as u32).to_le_bytes());
                        header.extend_from_slice(&(chunk_data.len() as u32).to_le_bytes());
                        header.extend_from_slice(hash.as_bytes());

                        stream.write_all(&header).await.ok();
                        stream.write_all(chunk_data).await.ok();
                        stream.finish().ok();
                    },
                    Err(e) => eprintln!("[QUIC] Failed to open stream for chunk {}: {}", chunk_id, e),
                }
            }));
        }

        // Wait for all
        for j in joins {
            j.await?;
        }
        
        // Graceful close
        conn_arc.close(0u32.into(), b"done");

        Ok(meta.size)
    }
}
