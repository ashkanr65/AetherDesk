use std::collections::VecDeque;

use ad_stream_protocol::{ArppFrame, MessagePriority};
use thiserror::Error;

pub struct PriorityQueue {
    highest: VecDeque<ArppFrame>,
    high: VecDeque<ArppFrame>,
    medium: VecDeque<ArppFrame>,
    low: VecDeque<ArppFrame>,
    per_class_capacity: usize,
}

impl PriorityQueue {
    pub fn new(per_class_capacity: usize) -> Result<Self, QueueError> {
        if per_class_capacity == 0 {
            return Err(QueueError::InvalidCapacity);
        }
        Ok(Self {
            highest: VecDeque::with_capacity(per_class_capacity),
            high: VecDeque::with_capacity(per_class_capacity),
            medium: VecDeque::with_capacity(per_class_capacity),
            low: VecDeque::with_capacity(per_class_capacity),
            per_class_capacity,
        })
    }

    pub fn push(&mut self, frame: ArppFrame) -> Result<Option<ArppFrame>, QueueError> {
        let priority = frame.priority();
        let queue = match priority {
            MessagePriority::Highest => &mut self.highest,
            MessagePriority::High => &mut self.high,
            MessagePriority::Medium => &mut self.medium,
            MessagePriority::Low => &mut self.low,
        };
        if queue.len() == self.per_class_capacity {
            return Err(QueueError::QueueFull(priority));
        }
        queue.push_back(frame);
        Ok(None)
    }

    pub fn pop(&mut self) -> Option<ArppFrame> {
        self.highest
            .pop_front()
            .or_else(|| self.high.pop_front())
            .or_else(|| self.medium.pop_front())
            .or_else(|| self.low.pop_front())
    }

    pub fn is_empty(&self) -> bool {
        self.highest.is_empty()
            && self.high.is_empty()
            && self.medium.is_empty()
            && self.low.is_empty()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum QueueError {
    #[error("ARPP queue capacity must be greater than zero")]
    InvalidCapacity,
    #[error("ARPP {0:?} queue is full; messages are never silently discarded")]
    QueueFull(MessagePriority),
}

#[cfg(test)]
mod tests {
    use ad_stream_protocol::{ArppMessage, DisplayMessage, InputMessage, Rectangle};

    use super::*;

    #[test]
    fn input_always_precedes_display_updates() {
        let mut queue = PriorityQueue::new(8).unwrap();
        queue
            .push(ArppFrame::new(
                1,
                ArppMessage::Display(DisplayMessage::DamageRegions {
                    frame_id: 1,
                    regions: vec![Rectangle {
                        x: 0,
                        y: 0,
                        width: 10,
                        height: 10,
                    }],
                }),
            ))
            .unwrap();
        queue
            .push(ArppFrame::new(
                2,
                ArppMessage::Input(InputMessage::PointerMove {
                    x: 4,
                    y: 5,
                    monitor: 0,
                }),
            ))
            .unwrap();
        assert_eq!(queue.pop().unwrap().sequence, 2);
        assert_eq!(queue.pop().unwrap().sequence, 1);
    }
}
