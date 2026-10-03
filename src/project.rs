use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const PROJECT_VERSION: u32 = 1;

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
        if self.version != PROJECT_VERSION {
            return Err(format!(
                "unsupported project version {}; expected {}",
                self.version, PROJECT_VERSION
            ));
        }
        if !self.bpm.is_finite() || !(20.0..=400.0).contains(&self.bpm) {
            return Err("bpm must be finite and between 20 and 400".to_owned());
        }

        let mut names = HashSet::with_capacity(self.tracks.len());
        for track in &self.tracks {
            let name = track.name.trim();
            if name.is_empty() {
                return Err("track names may not be empty".to_owned());
            }
            if !names.insert(name.to_owned()) {
                return Err(format!("duplicate track name: {name}"));
            }
        }

        Ok(())
    }
}
