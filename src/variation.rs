//! Spec 12 — invariant-locked pattern variation (control-thread only).

use serde::{Deserialize, Serialize};

pub const INVARIANT_SCHEMA_VERSION: u32 = 1;

/// Optional invariant profile persisted inside Pattern JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvariantProfile {
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<InvariantRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvariantRule {
    pub id: String,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: InvariantKind,
}

fn enabled_default() -> bool {
    true
}

/// Invariant kinds. Step positions in `anchor_steps` are one-based.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InvariantKind {
    AnchorSteps { steps: Vec<u16> },
    ActivityMask,
    PitchContour,
    EventCount,
    NoteRange { low: u8, high: u8 },
}

impl InvariantProfile {
    pub fn validate(&self, pattern_len: usize) -> Result<(), String> {
        let _ = pattern_len;
        if self.version != INVARIANT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported invariant schema version {}; expected {INVARIANT_SCHEMA_VERSION}",
                self.version
            ));
        }
        Ok(())
    }
}
