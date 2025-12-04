use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;

pub const CHUNK_SIZE: u64 = 1 * 1024 * 1024; //4 mb chunks

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub filename: String,
    pub size: u64,
    pub chunk_count: u64,
    pub hash: String,
}

impl FileMetadata {
    pub fn from_file(path: &str) -> Result<Self> {
        let meta = fs::metadata(path)?;
        let size = meta.len();
        let chunk_count = (size + CHUNK_SIZE - 1) / CHUNK_SIZE;

        // Compute SHA256 hash
        let mut file = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let hash = format!("{:x}", hasher.finalize());

        Ok(Self {
            filename: path.into(),
            size,
            chunk_count,
            hash,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn deserialize_metadata(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
