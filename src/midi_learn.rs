//! TEMPORARY stub; replaced by the MIDI learn module.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiMapping {}
impl MidiMapping {
    pub fn validate(&self) -> Result<(), String> {
        Ok(())
    }
}
