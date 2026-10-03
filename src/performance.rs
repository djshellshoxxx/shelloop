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
    pub fn map(&self, _normalized: f32) -> f32 {
        self.min
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XyPoint {
    pub x: f32,
    pub y: f32,
}

impl XyPoint {
    pub fn from_terminal(column: u16, row: u16, width: u16, height: u16) -> Self {
        let _ = (column, row, width, height);
        Self { x: 0.0, y: 0.0 }
    }
}
