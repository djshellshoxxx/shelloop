#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizeBoundary {
    Immediate,
    Step,
    Beat,
    Bar { beats_per_bar: u32 },
    Pattern { steps: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizedChange<T> {
    pub apply_at_frame: u64,
    pub value: T,
}

impl<T> QuantizedChange<T> {
    pub fn new(
        value: T,
        current_frame: u64,
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        boundary: QuantizeBoundary,
    ) -> Result<Self, String> {
        Ok(Self {
            apply_at_frame: next_boundary_frame(
                current_frame,
                sample_rate,
                bpm,
                steps_per_beat,
                boundary,
            )?,
            value,
        })
    }
}

pub fn next_boundary_frame(
    current_frame: u64,
    sample_rate: u32,
    bpm: f64,
    steps_per_beat: u32,
    boundary: QuantizeBoundary,
) -> Result<u64, String> {
    validate_clock(sample_rate, bpm, steps_per_beat)?;
    if boundary == QuantizeBoundary::Immediate {
        return Ok(current_frame);
    }

    let frames_per_beat = sample_rate as f64 * 60.0 / bpm;
    let frames_per_step = frames_per_beat / steps_per_beat as f64;
    let interval = match boundary {
        QuantizeBoundary::Immediate => unreachable!(),
        QuantizeBoundary::Step => frames_per_step,
        QuantizeBoundary::Beat => frames_per_beat,
        QuantizeBoundary::Bar { beats_per_bar } => {
            if beats_per_bar == 0 || beats_per_bar > 64 {
                return Err("beats per bar must be between 1 and 64".into());
            }
            frames_per_beat * beats_per_bar as f64
        }
        QuantizeBoundary::Pattern { steps } => {
            if steps == 0 || steps > 256 {
                return Err("pattern steps must be between 1 and 256".into());
            }
            frames_per_step * steps as f64
        }
    };

    if !interval.is_finite() || interval <= 0.0 {
        return Err("quantization interval must be finite and positive".into());
    }

    Ok(next_grid_frame(current_frame, interval))
}

fn validate_clock(sample_rate: u32, bpm: f64, steps_per_beat: u32) -> Result<(), String> {
    if sample_rate == 0 {
        return Err("sample rate must be greater than zero".into());
    }
    if !bpm.is_finite() || !(20.0..=400.0).contains(&bpm) {
        return Err("bpm must be finite and between 20 and 400".into());
    }
    if steps_per_beat == 0 || steps_per_beat > 64 {
        return Err("steps per beat must be between 1 and 64".into());
    }
    Ok(())
}

fn next_grid_frame(current_frame: u64, interval_frames: f64) -> u64 {
    let approximate_index = (current_frame as f64 / interval_frames).floor() as u64;
    let current_boundary = boundary_frame(approximate_index, interval_frames);
    if current_boundary >= current_frame {
        return current_boundary;
    }

    let mut index = approximate_index.saturating_add(1);
    let mut candidate = boundary_frame(index, interval_frames);
    while candidate < current_frame {
        index = index.saturating_add(1);
        let next = boundary_frame(index, interval_frames);
        if next == candidate {
            return current_frame;
        }
        candidate = next;
    }
    candidate
}

fn boundary_frame(index: u64, interval_frames: f64) -> u64 {
    let frame = index as f64 * interval_frames;
    if !frame.is_finite() || frame >= u64::MAX as f64 {
        u64::MAX
    } else {
        frame.round() as u64
    }
}
