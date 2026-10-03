use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub version: u32,
    pub bpm: f64,
    pub tracks: Vec<Track>,
}

impl Project {
    pub fn validate(&self) -> Result<(), String> {
        Err("validation not implemented".to_owned())
    }
}
