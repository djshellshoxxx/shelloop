//! Auto-save policy (pure). Writing happens in the controller through the
//! existing atomic project save, to a sidecar file so the project file itself
//! is only replaced by an explicit `save`.

pub const DEFAULT_AUTOSAVE_INTERVAL_MS: u64 = 60_000;
pub const MIN_AUTOSAVE_INTERVAL_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutosavePolicy {
    enabled: bool,
    interval_ms: u64,
    last_saved_ms: Option<u64>,
}

impl Default for AutosavePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_ms: DEFAULT_AUTOSAVE_INTERVAL_MS,
            last_saved_ms: None,
        }
    }
}

impl AutosavePolicy {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_interval_ms(&mut self, interval_ms: u64) -> Result<(), String> {
        if interval_ms < MIN_AUTOSAVE_INTERVAL_MS {
            return Err(format!(
                "autosave interval must be at least {} seconds",
                MIN_AUTOSAVE_INTERVAL_MS / 1000
            ));
        }
        self.interval_ms = interval_ms;
        Ok(())
    }

    /// Due when enabled, there are unsaved edits, a project path exists and
    /// the interval has elapsed since the last autosave (or since start).
    pub fn due(&self, now_ms: u64, dirty: bool, has_path: bool) -> bool {
        if !self.enabled || !dirty || !has_path {
            return false;
        }
        match self.last_saved_ms {
            None => now_ms >= self.interval_ms,
            Some(last) => now_ms.saturating_sub(last) >= self.interval_ms,
        }
    }

    pub fn mark_saved(&mut self, now_ms: u64) {
        self.last_saved_ms = Some(now_ms);
    }
}

/// Sidecar path for autosaves: `song.json` becomes `song.autosave.json`.
pub fn autosave_path(project: &std::path::Path) -> std::path::PathBuf {
    project.with_extension("autosave.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn due_respects_interval_dirty_flag_and_path() {
        let mut policy = AutosavePolicy::default();
        assert!(!policy.due(59_999, true, true));
        assert!(policy.due(60_000, true, true));
        assert!(!policy.due(60_000, false, true));
        assert!(!policy.due(60_000, true, false));
        policy.mark_saved(60_000);
        assert!(!policy.due(90_000, true, true));
        assert!(policy.due(120_000, true, true));
        policy.set_enabled(false);
        assert!(!policy.due(999_999, true, true));
    }

    #[test]
    fn interval_has_a_floor() {
        let mut policy = AutosavePolicy::default();
        assert!(policy.set_interval_ms(1_000).is_err());
        assert!(policy.set_interval_ms(5_000).is_ok());
    }

    #[test]
    fn sidecar_path_keeps_the_directory() {
        assert_eq!(
            autosave_path(Path::new("/x/song.json")),
            Path::new("/x/song.autosave.json")
        );
    }
}
