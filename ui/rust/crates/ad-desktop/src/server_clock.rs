//! The status clock is a server response, never an extrapolated local clock.
use eframe::egui;
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
const FRESH: Duration = Duration::from_secs(3);
// One non-PTY exec channel per connection. No terminal history, detached
// process or periodic SSH connection; EOF terminates the server read loop.
const COMMAND:&str="exec sh -c 'while IFS= read -r pulse; do [ \"$pulse\" = clock ] || exit; LC_ALL=C date +%H:%M:%S || exit; done'";
#[derive(Clone, Default)]
pub(crate) struct Clock(Arc<Mutex<Option<(String, Instant)>>>);
impl Clock {
    pub(crate) fn text(&self, connected: bool) -> String {
        self.at(connected, Instant::now())
    }
    fn at(&self, connected: bool, now: Instant) -> String {
        if connected {
            if let Some((text, received)) = &*self.0.lock() {
                if now.saturating_duration_since(*received) < FRESH {
                    return text.clone();
                }
            }
        }
        "--:--:--".into()
    }
    fn clear(&self) {
        *self.0.lock() = None;
    }
}
fn parse(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 9
        || bytes[2] != b':'
        || bytes[5] != b':'
        || bytes[8] != b'\n'
        || ![0, 1, 3, 4, 6, 7]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
    {
        return None;
    }
    let number = |i: usize| u16::from(bytes[i] - b'0') * 10 + u16::from(bytes[i + 1] - b'0');
    if number(0) > 23 || number(3) > 59 || number(6) > 60 {
        return None;
    }
    std::str::from_utf8(&bytes[..8]).ok().map(str::to_owned)
}
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> Result<String, &'static str> {
    stream
        .write_all(b"clock\n")
        .await
        .map_err(|_| "Clock connection closed")?;
    stream
        .flush()
        .await
        .map_err(|_| "Clock connection closed")?;
    let mut bytes = [0; 9];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "Clock connection closed")?;
    parse(&bytes).ok_or("Invalid server clock response")
}
pub(crate) struct Lease {
    cancel: CancellationToken,
    clock: Clock,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.clock.clear();
    }
}
impl Lease {
    pub(crate) fn start(
        session: ad_ssh::SshSession,
        clock: Clock,
        ctx: egui::Context,
        parent: &CancellationToken,
    ) -> Self {
        let cancel = parent.child_token();
        let worker_cancel = cancel.clone();
        let output = clock.clone();
        tokio::spawn(async move {
            while !worker_cancel.is_cancelled() {
                let channel = tokio::select! {_ = worker_cancel.cancelled()=>break,
                channel=tokio::time::timeout(FRESH,session.open_exec_channel(COMMAND))=>channel};
                if let Ok(Ok(channel)) = channel {
                    let mut stream = channel.into_stream();
                    let mut interval = tokio::time::interval(Duration::from_secs(1));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tokio::select! {_ = worker_cancel.cancelled()=>break,_ = interval.tick()=>{}}
                        let reply = tokio::select! {_ = worker_cancel.cancelled()=>break,
                        reply=tokio::time::timeout(FRESH,exchange(&mut stream))=>reply};
                        match reply {
                            Ok(Ok(text)) => {
                                *output.0.lock() = Some((text, Instant::now()));
                                ctx.request_repaint();
                            }
                            _ => break,
                        }
                    }
                    let _ =
                        tokio::time::timeout(Duration::from_millis(250), stream.shutdown()).await;
                }
                output.clear();
                ctx.request_repaint();
                tokio::select! {_ = worker_cancel.cancelled()=>break,_ = tokio::time::sleep(Duration::from_secs(2))=>{}}
            }
            output.clear();
            ctx.request_repaint();
        });
        Self { cancel, clock }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_displays_only_fresh_server_values_and_clears_on_disconnect() {
        let clock = Clock::default();
        let now = Instant::now();
        assert_eq!(clock.at(true, now), "--:--:--");
        *clock.0.lock() = Some(("04:05:06".into(), now));
        assert_eq!(clock.at(true, now + Duration::from_secs(2)), "04:05:06");
        assert_eq!(clock.at(true, now + FRESH), "--:--:--");
        assert_eq!(clock.at(false, now), "--:--:--");
        for bytes in [
            b"24:00:00\n".as_slice(),
            b"12:60:00\n",
            b"not time\n",
            b"12:34:56x",
            b"12:34:56\nextra",
        ] {
            assert!(parse(bytes).is_none());
        }
        assert_eq!(parse(b"23:59:59\n").unwrap(), "23:59:59");
    }
    #[tokio::test]
    async fn clock_uses_server_response_and_closes_its_non_terminal_channel() {
        let (mut client, mut server) = tokio::io::duplex(32);
        let task = tokio::spawn(async move {
            let mut request = [0; 6];
            server.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"clock\n");
            server.write_all(b"03:02:01\n").await.unwrap();
        });
        assert_eq!(exchange(&mut client).await.unwrap(), "03:02:01");
        task.await.unwrap();
        assert!(exchange(&mut client).await.is_err());
        let clock = Clock::default();
        *clock.0.lock() = Some(("01:02:03".into(), Instant::now()));
        let cancel = CancellationToken::new();
        let lease = Lease {
            cancel: cancel.clone(),
            clock: clock.clone(),
        };
        drop(lease);
        assert!(cancel.is_cancelled());
        assert_eq!(clock.text(true), "--:--:--");
        assert!(!COMMAND.contains("sleep"));
        assert!(COMMAND.contains("read -r pulse"));
    }
}
