use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;



#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub filename: String,
    pub size: u64,
    pub chunk_count: u64,
    pub chunk_size: u64,
}

impl FileMetadata {
    pub fn from_file(path: &str) -> Result<Self> {
        let meta = fs::metadata(path)?;
        let size = meta.len();
        
        // Calculate optimal chunk size
        let chunk_size = if size > 10 * 1024 * 1024 * 1024 {
            // > 10GB -> 64MB chunks
            64 * 1024 * 1024
        } else if size > 1 * 1024 * 1024 * 1024 {
            // > 1GB -> 16MB chunks
            16 * 1024 * 1024
        } else if size > 100 * 1024 * 1024 {
             // > 100MB -> 4MB chunks
            4 * 1024 * 1024
        } else {
             // < 100MB -> 1MB chunks
            1 * 1024 * 1024
        };

        // Calculate chunk count based on dynamic chunk size
        let chunk_count = (size + chunk_size - 1) / chunk_size;

        // NOTE: We removed the global hash calculation here for performance.
        // Verification is now done per-chunk using BLAKE3.

        Ok(Self {
            filename: path.into(),
            size,
            chunk_count,
            chunk_size,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn deserialize_metadata(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
