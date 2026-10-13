use std::collections::VecDeque;

use ad_stream_protocol::InputMessage;
use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct InputQueue {
    state: Mutex<State>,
    ready: Notify,
}

#[derive(Default)]
struct State {
    events: VecDeque<InputMessage>,
    failed: bool,
}

impl InputQueue {
    pub fn push(&self, event: InputMessage) -> Result<(), String> {
        let mut state = self.state.lock();
        if state.failed {
            return Err("Remote input stopped; reconnect the desktop".into());
        }
        // Only adjacent positions may collapse: a click, key, or wheel is an
        // ordering barrier, so drags and click locations stay correct.
        if matches!(event, InputMessage::PointerMove { .. })
            && matches!(state.events.back(), Some(InputMessage::PointerMove { .. }))
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

    pub async fn receive(&self) -> Result<InputMessage, String> {
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
            InputMessage::PointerMove { x: 9999, .. }
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            InputMessage::Keyboard { pressed: true, .. }
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            InputMessage::PointerMove { x: 9999, .. }
        ));
        assert!(matches!(
            queue.receive().await.unwrap(),
            InputMessage::Keyboard { pressed: false, .. }
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
}
