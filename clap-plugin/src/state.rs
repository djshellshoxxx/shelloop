//! Versioned JSON state blob holding parameter values.
//!
//! ```json
//! {"format":"shelloop-clap","version":1,"params":{"oscillator":2.0,...}}
//! ```
//!
//! Values are keyed by the stable [`ParamSpec::key`](crate::params::ParamSpec)
//! names. Unknown keys are ignored and missing keys fall back to defaults so
//! older and newer blobs both load.

use crate::params::{default_values, ParamValues, PARAMS};
use serde_json::{Map, Value};

pub const STATE_FORMAT: &str = "shelloop-clap";
pub const STATE_VERSION: u64 = 1;
/// Upper bound on accepted state size; a real blob is well under 1 KiB.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

pub fn encode(values: &ParamValues) -> Vec<u8> {
    let mut params = Map::new();
    for spec in &PARAMS {
        params.insert(spec.key.to_owned(), Value::from(values[spec.id.index()]));
    }
    let mut root = Map::new();
    root.insert("format".to_owned(), Value::from(STATE_FORMAT));
    root.insert("version".to_owned(), Value::from(STATE_VERSION));
    root.insert("params".to_owned(), Value::Object(params));
    serde_json::to_vec(&Value::Object(root)).unwrap_or_default()
}

pub fn decode(bytes: &[u8]) -> Result<ParamValues, String> {
    let root: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid state JSON: {error}"))?;
    if root.get("format").and_then(Value::as_str) != Some(STATE_FORMAT) {
        return Err("state blob is not a SHELLOOP CLAP state".into());
    }
    match root.get("version").and_then(Value::as_u64) {
        Some(version) if (1..=STATE_VERSION).contains(&version) => {}
        Some(version) => return Err(format!("unsupported state version {version}")),
        None => return Err("state blob has no version".into()),
    }
    let params = root
        .get("params")
        .and_then(Value::as_object)
        .ok_or("state blob has no params object")?;

    let mut values = default_values();
    for spec in &PARAMS {
        if let Some(value) = params.get(spec.key).and_then(Value::as_f64) {
            if let Some(value) = spec.sanitize(value) {
                values[spec.id.index()] = value;
            }
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamId;

    #[test]
    fn round_trips_values() {
        let mut values = default_values();
        values[ParamId::Oscillator.index()] = 3.0;
        values[ParamId::Cutoff.index()] = 523.25;
        values[ParamId::Sequencer.index()] = 1.0;
        assert_eq!(decode(&encode(&values)).unwrap(), values);
    }

    #[test]
    fn tolerates_missing_unknown_and_out_of_range_keys() {
        let blob =
            br#"{"format":"shelloop-clap","version":1,"params":{"octave":99,"future_knob":1}}"#;
        let values = decode(blob).unwrap();
        assert_eq!(values[ParamId::Octave.index()], 4.0);
        assert_eq!(
            values[ParamId::Cutoff.index()],
            ParamId::Cutoff.spec().default
        );
    }

    #[test]
    fn rejects_foreign_or_future_blobs() {
        assert!(decode(b"not json").is_err());
        assert!(decode(br#"{"format":"other","version":1,"params":{}}"#).is_err());
        assert!(decode(br#"{"format":"shelloop-clap","version":2,"params":{}}"#).is_err());
    }
}
