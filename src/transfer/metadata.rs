use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024; //4 mb chunks

#[derive(Serialize, Deserialize, Debug)]
pub struct FileMetadata {
    pub filename: String,
    pub size: u64,
    pub chunk_count: u64,
}

impl FileMetadata {
    pub fn from_file(path: &str) -> Result<Self> {
        let meta = fs::metadata(path)?;
        let size = meta.len();
        let chunk_count = (size + CHUNK_SIZE - 1) / CHUNK_SIZE;

        Ok(Self {
            filename: path.into(),
            size,
            chunk_count,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn deserialize_metadata(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
