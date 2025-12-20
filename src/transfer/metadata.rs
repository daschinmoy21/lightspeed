use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024; //4 mb chunks

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub filename: String,
    pub size: u64,
    pub chunk_count: u64,
}

impl FileMetadata {
    pub fn from_file(path: &str) -> Result<Self> {
        // LEARN: standard fs::metadata call to get file size
        // This is a synchronous (blocking) call. In a highly concurrent async context,
        // you might want to wrap this in tokio::task::spawn_blocking to avoid blocking the runtime thread.
        let meta = fs::metadata(path)?;
        let size = meta.len();
        // Calculate how many chunks we need based on the constant chunk size
        let chunk_count = (size + CHUNK_SIZE - 1) / CHUNK_SIZE;

        // NOTE: We removed the global hash calculation here for performance.
        // Verification is now done per-chunk using BLAKE3.

        Ok(Self {
            filename: path.into(),
            size,
            chunk_count,
            // hash: String::new(), // Field removed from struct
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn deserialize_metadata(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
