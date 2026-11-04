//! Named, versioned synth patches. File operations run on the control thread;
//! loading returns a validated value for preparation before audio handoff.
use crate::SynthPatch;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const SCHEMA: u32 = 1;
const MAX_BYTES: u64 = 65_536;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    schema_version: u32,
    name: String,
    patch: SynthPatch,
}

#[derive(Debug, Clone)]
pub struct PresetStore {
    directory: PathBuf,
}

impl PresetStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err("preset names require 1–64 letters, digits, underscores or hyphens".into());
        }
        Ok(self.directory.join(format!("{name}.json")))
    }

    pub fn list(&self) -> Result<Vec<String>, String> {
        if !self.directory.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.directory).map_err(|e| format!("read presets: {e}"))? {
            let entry = entry.map_err(|e| format!("read preset entry: {e}"))?;
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("json") {
                if let Some(name) = path.file_stem().and_then(|x| x.to_str()) {
                    if self.path(name).is_ok()
                        && entry.file_type().map_err(|e| e.to_string())?.is_file()
                    {
                        names.push(name.to_owned());
                    }
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn load(&self, name: &str, sample_rate: f32) -> Result<SynthPatch, String> {
        let path = self.path(name)?;
        let mut data = Vec::new();
        std::fs::File::open(&path)
            .map_err(|e| format!("read preset: {e}"))?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("read preset: {e}"))?;
        if data.len() as u64 > MAX_BYTES {
            return Err("preset exceeds the 64 KiB limit".into());
        }
        let preset: Preset =
            serde_json::from_slice(&data).map_err(|e| format!("parse preset: {e}"))?;
        if preset.schema_version != SCHEMA || preset.name != name {
            return Err("unsupported preset schema or mismatched name".into());
        }
        preset.patch.validate(sample_rate)?;
        Ok(preset.patch)
    }

    /// Atomically publish a new preset; existing named files are never replaced.
    pub fn save(&self, name: &str, patch: SynthPatch, sample_rate: f32) -> Result<(), String> {
        let target = self.path(name)?;
        patch.validate(sample_rate)?;
        let data = serde_json::to_vec_pretty(&Preset {
            schema_version: SCHEMA,
            name: name.to_owned(),
            patch,
        })
        .map_err(|e| format!("serialize preset: {e}"))?;
        fs::create_dir_all(&self.directory).map_err(|e| format!("create presets: {e}"))?;
        let temp = self.directory.join(format!(
            ".preset-{}-{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("create preset: {e}"))?;
        let result = (|| {
            file.write_all(&data)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("write preset: {e}"))?;
            drop(file);
            fs::hard_link(&temp, target)
                .map_err(|e| format!("save new preset (existing names are protected): {e}"))
        })();
        let _ = fs::remove_file(temp);
        result
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        fs::remove_file(self.path(name)?).map_err(|e| format!("delete preset: {e}"))
    }
}
