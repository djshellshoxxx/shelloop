#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisCurve {
    Linear,
    Logarithmic,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisMapping {
    pub min: f32,
    pub max: f32,
    pub inverted: bool,
    pub curve: AxisCurve,
}

impl AxisMapping {
    pub fn map(&self, normalized: f32) -> f32 {
        let mut t = if normalized.is_finite() {
            normalized.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.inverted {
            t = 1.0 - t;
        }

        match self.curve {
            AxisCurve::Linear => self.min + (self.max - self.min) * t,
            AxisCurve::Logarithmic if self.min > 0.0 && self.max > 0.0 => {
                let min_ln = self.min.ln();
                let max_ln = self.max.ln();
                (min_ln + (max_ln - min_ln) * t).exp()
            }
            AxisCurve::Logarithmic => self.min + (self.max - self.min) * t,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XyPoint {
    pub x: f32,
    pub y: f32,
}

impl XyPoint {
    pub fn from_terminal(column: u16, row: u16, width: u16, height: u16) -> Self {
        let x_denom = width.saturating_sub(1).max(1) as f32;
        let y_denom = height.saturating_sub(1).max(1) as f32;
        let x = (column.min(width.saturating_sub(1)) as f32 / x_denom).clamp(0.0, 1.0);
        let screen_y = (row.min(height.saturating_sub(1)) as f32 / y_denom).clamp(0.0, 1.0);
        Self {
            x,
            y: 1.0 - screen_y,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerformanceMix {
    pub live_gain: f32,
    pub sequencer_gain: f32,
}

impl PerformanceMix {
    pub const UNITY: Self = Self {
        live_gain: 1.0,
        sequencer_gain: 1.0,
    };

    pub fn from_xy(point: XyPoint, has_sequencer: bool) -> Self {
        let level = if point.y.is_finite() {
            point.y.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !has_sequencer {
            return Self {
                live_gain: level,
                sequencer_gain: 0.0,
            };
        }

        let crossfade = if point.x.is_finite() {
            point.x.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            live_gain: (1.0 - crossfade) * level,
            sequencer_gain: crossfade * level,
        }
    }
}
