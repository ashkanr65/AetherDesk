//! Native clipboard APIs may wait for another process during lock/wake.
//! One bounded process-wide worker keeps those waits off the GUI thread.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, OnceLock,
};
const LIMIT: usize = 1024 * 1024;
type Reply = Result<Option<String>, String>;
enum Action {
    Read,
    Write(String),
}
struct Job {
    action: Action,
    reply: mpsc::SyncSender<Reply>,
    alive: Arc<AtomicBool>,
}
fn service() -> Option<&'static mpsc::SyncSender<Job>> {
    static SERVICE: OnceLock<Option<mpsc::SyncSender<Job>>> = OnceLock::new();
    SERVICE
        .get_or_init(|| {
            let (send, receive) = mpsc::sync_channel::<Job>(8);
            std::thread::Builder::new()
                .name("remote-clipboard".into())
                .spawn(move || {
                    let mut clipboard = None;
                    run_jobs(receive, move |action| {
                        if clipboard.is_none() {
                            clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
                        }
                        let clipboard = clipboard.as_mut().unwrap();
                        match action {
                            Action::Read => clipboard
                                .get_text()
                                .map(|text| (text.len() <= LIMIT).then_some(text))
                                .map_err(|e| e.to_string()),
                            Action::Write(text) => clipboard
                                .set_text(text)
                                .map(|_| None)
                                .map_err(|e| e.to_string()),
                        }
                    });
                })
                .ok()
                .map(|_| send)
        })
        .as_ref()
}
fn run_jobs(receive: mpsc::Receiver<Job>, mut native: impl FnMut(Action) -> Reply) {
    while let Ok(job) = receive.recv() {
        if !job.alive.load(Ordering::Acquire) {
            continue;
        }
        let result = native(job.action);
        let _ = job.reply.try_send(result);
    }
}
fn submit(
    send: &mpsc::SyncSender<Job>,
    action: Action,
    alive: Arc<AtomicBool>,
) -> Result<mpsc::Receiver<Reply>, Action> {
    let (reply, receive) = mpsc::sync_channel(1);
    send.try_send(Job {
        action,
        reply,
        alive,
    })
    .map(|_| receive)
    .map_err(|e| match e {
        mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job.action,
    })
}
pub(crate) struct Clipboard {
    pending: Option<(mpsc::Receiver<Reply>, Option<String>)>,
    write: Option<String>,
    alive: Arc<AtomicBool>,
    retry_at: std::time::Instant,
}
impl Default for Clipboard {
    fn default() -> Self {
        Self {
            pending: None,
            write: None,
            alive: Arc::new(AtomicBool::new(true)),
            retry_at: std::time::Instant::now(),
        }
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}
impl Clipboard {
    pub(crate) fn poll(&mut self, remote: Option<String>, focused: bool) -> Option<String> {
        if let Some(text) = remote.filter(|text| text.len() <= LIMIT) {
            self.write = Some(text);
        }
        if let Some((pending, retry)) = self.pending.take() {
            match pending.try_recv() {
                Ok(Ok(text)) => {
                    self.pending = None;
                    if self.write.is_none() && focused && text.is_some() {
                        return text;
                    }
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    if self.write.is_none() {
                        self.write = retry;
                    }
                    self.retry_at = std::time::Instant::now() + std::time::Duration::from_secs(2);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.pending = Some((pending, retry));
                    return None;
                }
            }
        }
        if std::time::Instant::now() < self.retry_at {
            return None;
        }
        let Some(send) = service() else {
            return None;
        };
        let action = if let Some(text) = self.write.take() {
            Action::Write(text)
        } else if focused {
            Action::Read
        } else {
            return None;
        };
        let retry = match &action {
            Action::Write(text) => Some(text.clone()),
            Action::Read => None,
        };
        match submit(send, action, self.alive.clone()) {
            Ok(receive) => self.pending = Some((receive, retry)),
            Err(Action::Write(text)) => self.write = Some(text),
            Err(Action::Read) => {}
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_native_clipboard_cannot_block_submission_or_accumulate_unbounded_jobs() {
        let (send, receive) = mpsc::sync_channel(1);
        let (entered, started) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_jobs(receive, |_| {
                entered.send(()).unwrap();
                paused.recv().unwrap();
                Ok(Some("complete".into()))
            })
        });
        let alive = Arc::new(AtomicBool::new(true));
        let first = submit(&send, Action::Read, alive.clone())
            .unwrap_or_else(|_| panic!("first job rejected"));
        started
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(first.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let queued = submit(&send, Action::Read, alive.clone())
            .unwrap_or_else(|_| panic!("queued job rejected"));
        assert!(submit(&send, Action::Read, alive.clone()).is_err());
        alive.store(false, Ordering::Release);
        resume.send(()).unwrap();
        assert_eq!(
            first
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            Some("complete".into())
        );
        drop(send);
        worker.join().unwrap();
        assert!(
            queued.recv().is_err(),
            "retired view's queued clipboard job ran"
        );
    }
}
