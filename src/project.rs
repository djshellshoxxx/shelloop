use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::Path,
};

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

    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), String> {
        self.validate()?;
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| format!("create project directory: {error}"))?;

        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(|value| format!("{value}.tmp"))
            .unwrap_or_else(|| "tmp".to_owned());
        let temp_path = path.with_extension(extension);

        let result = (|| -> Result<(), String> {
            let json = serde_json::to_vec_pretty(self)
                .map_err(|error| format!("serialize project: {error}"))?;
            let mut file = File::create(&temp_path)
                .map_err(|error| format!("create temporary project: {error}"))?;
            file.write_all(&json)
                .map_err(|error| format!("write temporary project: {error}"))?;
            file.write_all(b"\n")
                .map_err(|error| format!("finish temporary project: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("sync temporary project: {error}"))?;
            fs::rename(&temp_path, path).map_err(|error| format!("replace project file: {error}"))?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes = fs::read(path.as_ref()).map_err(|error| format!("read project: {error}"))?;
        let project: Self =
            serde_json::from_slice(&bytes).map_err(|error| format!("parse project: {error}"))?;
        project.validate()?;
        Ok(project)
    }
}
