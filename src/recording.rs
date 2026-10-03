use std::collections::VecDeque;

#[derive(Debug)]
pub struct RecordingQueue {
    capacity_blocks: usize,
    blocks: VecDeque<Vec<f32>>,
    dropped_blocks: u64,
}

impl RecordingQueue {
    pub fn new(capacity_blocks: usize) -> Self {
        Self {
            capacity_blocks,
            blocks: VecDeque::with_capacity(capacity_blocks),
            dropped_blocks: 0,
        }
    }

    pub fn push(&mut self, block: Vec<f32>) -> bool {
        if self.blocks.len() >= self.capacity_blocks {
            self.dropped_blocks = self.dropped_blocks.saturating_add(1);
            return false;
        }
        self.blocks.push_back(block);
        true
    }

    pub fn pop(&mut self) -> Option<Vec<f32>> {
        self.blocks.pop_front()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn capacity_blocks(&self) -> usize {
        self.capacity_blocks
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks
    }
}
