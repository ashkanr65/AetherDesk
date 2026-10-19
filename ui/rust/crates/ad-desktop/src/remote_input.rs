use std::collections::VecDeque;

use ad_stream_protocol::{ArppMessage, ControlMessage, InputMessage};
use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct InputQueue {
    state: Mutex<State>,
    ready: Notify,
}

#[derive(Default)]
struct State {
    events: VecDeque<ArppMessage>,
    failed: bool,
}

impl InputQueue {
    pub fn push(&self, event: InputMessage) -> Result<(), String> {
        self.push_message(ArppMessage::Input(event))
    }
    pub fn control(&self, event: ControlMessage) -> Result<(), String> {
        // Small configuration barriers belong before subsequent key/pointer
        // edges, rather than racing a separate control worker.
        let valid = match &event {
            ControlMessage::KeyboardLayout { layout, variant } => {
                ad_stream_protocol::valid_keyboard_layout(layout, variant)
            }
            ControlMessage::MonitorLayout {
                width,
                height,
                monitors,
            } => ad_stream_protocol::valid_monitor_layout(*width, *height, monitors),
            ControlMessage::MonitorLayoutWithProperties {
                width,
                height,
                monitors,
                properties,
            } => {
                ad_stream_protocol::valid_monitor_layout(*width, *height, monitors)
                    && ad_stream_protocol::valid_monitor_properties(monitors.len(), properties)
            }
            ControlMessage::Resize { width, height, .. } => {
                ad_stream_protocol::valid_desktop_size(*width, *height)
            }
            _ => false,
        };
        if !valid {
            return Err("Invalid remote input configuration barrier".into());
        }
        self.push_message(ArppMessage::Control(event))
    }
    fn push_message(&self, event: ArppMessage) -> Result<(), String> {
        let mut state = self.state.lock();
        if state.failed {
            return Err("Remote input stopped; reconnect the desktop".into());
        }
        // Only adjacent positions may collapse: a click, key, or wheel is an
        // ordering barrier, so drags and click locations stay correct.
        if matches!(event, ArppMessage::Input(InputMessage::PointerMove { .. }))
            && matches!(
                state.events.back(),
                Some(ArppMessage::Input(InputMessage::PointerMove { .. }))
            )
        {
            state.events.pop_back();
        }
        if state.events.len() >= 2048 {
            state.failed = true;
            self.ready.notify_one();
            return Err("Remote input stalled; disconnecting to release held keys".into());
        }
        state.events.push_back(event);
        self.ready.notify_one();
        Ok(())
    }

    pub async fn receive(&self) -> Result<ArppMessage, String> {
        loop {
            let ready = self.ready.notified();
            {
                let mut state = self.state.lock();
                if state.failed {
                    return Err("Remote input stalled; reconnect the desktop".into());
                }
                if let Some(event) = state.events.pop_front() {
                    return Ok(event);
                }
            }
            ready.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(x: i32) -> InputMessage {
        InputMessage::PointerMove {
            x,
            y: 0,
            monitor: 0,
        }
    }
    fn key(pressed: bool) -> InputMessage {
        InputMessage::Keyboard {
            scan_code: 30,
            pressed,
            modifiers: 0,
            repeat: false,
        }
    }

    #[tokio::test]
    async fn pointer_flood_preserves_key_edges_and_order() {
        let queue = InputQueue::default();
        for x in 0..10_000 {
            queue.push(point(x)).unwrap();
        }
        queue.push(key(true)).unwrap();
        for x in 0..10_000 {
            queue.push(point(x)).unwrap();
        }
        queue.push(key(false)).unwrap();
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::PointerMove { x: 9999, .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::Keyboard { pressed: true, .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::PointerMove { x: 9999, .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::Keyboard { pressed: false, .. })
        ));
    }

    #[tokio::test]
    async fn overload_fails_the_session_instead_of_silently_losing_a_release() {
        let queue = InputQueue::default();
        for _ in 0..2048 {
            queue.push(key(true)).unwrap();
        }
        assert!(queue.push(key(false)).is_err());
        assert!(queue.receive().await.is_err());
    }
    #[tokio::test]
    async fn layout_changes_are_ordered_barriers_for_keyboard_and_mouse() {
        let queue = InputQueue::default();
        queue.push(point(1)).unwrap();
        queue
            .control(ControlMessage::KeyboardLayout {
                layout: "de".into(),
                variant: String::new(),
            })
            .unwrap();
        queue.push(point(2)).unwrap();
        queue.push(key(true)).unwrap();
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::PointerMove { x: 1, .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Control(ControlMessage::KeyboardLayout { .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::PointerMove { x: 2, .. })
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            ArppMessage::Input(InputMessage::Keyboard { pressed: true, .. })
        ));
        assert!(queue
            .control(ControlMessage::KeyboardLayout {
                layout: "../../bad".into(),
                variant: String::new()
            })
            .is_err());
    }
}
