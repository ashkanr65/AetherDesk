//! Suppress only the echo of an application-owned directory command. Real
//! command output, shell errors and all bytes after the end marker remain.
use std::time::{Duration, Instant};
#[derive(Default)]
pub(crate) struct Navigation {
    queued: Option<String>,
    pending: Option<Pending>,
}
struct Pending {
    begin: Vec<u8>,
    end: Vec<u8>,
    bytes: Vec<u8>,
    started: Instant,
}
fn position(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|b| b == needle)
}
impl Navigation {
    pub(crate) fn request(&mut self, quoted_path: String) {
        self.queued = Some(quoted_path);
    }
    pub(crate) fn cancel_queued(&mut self) {
        self.queued = None;
    }
    pub(crate) fn command(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_some() {
            return None;
        }
        let path = self.queued.take()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.pending = Some(Pending {
            begin: format!("\x1b]777;{token};begin\x07").into_bytes(),
            end: format!("\x1b]777;{token};end\x07").into_bytes(),
            bytes: vec![],
            started: Instant::now(),
        });
        Some(format!(" __aetherdesk_cd {path} '{token}'\r").into_bytes())
    }
    pub(crate) fn filter(&mut self, bytes: &[u8]) -> Vec<u8> {
        let Some(pending) = &mut self.pending else {
            return bytes.to_vec();
        };
        pending.bytes.extend_from_slice(bytes);
        if let Some(begin) = position(&pending.bytes, &pending.begin) {
            let content = begin + pending.begin.len();
            if let Some(end) = position(&pending.bytes[content..], &pending.end) {
                let end = content + end;
                let mut result = b"\r\x1b[2K".to_vec(); // replace the existing prompt
                if let Some(echo) = position(&pending.bytes[..begin], b" __aetherdesk_cd ") {
                    result.extend_from_slice(&pending.bytes[..echo]);
                }
                result.extend_from_slice(&pending.bytes[content..end]);
                result.extend_from_slice(&pending.bytes[end + pending.end.len()..]);
                self.pending = None;
                return result;
            }
        }
        self.expire(Instant::now())
    }
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<u8> {
        if self.pending.as_ref().is_some_and(|p| {
            p.bytes.len() > 65536 || now.duration_since(p.started) > Duration::from_secs(2)
        }) {
            // An unexpected interactive program/shell must never hide output.
            return self.pending.take().unwrap().bytes;
        }
        vec![]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canceling_folder_sync_keeps_inflight_echo_filter_and_drops_unsent_changes() {
        let mut navigation = Navigation::default();
        navigation.request("'/inflight'".into());
        navigation.command().unwrap();
        let pending = navigation.pending.as_ref().unwrap();
        let mut wire = pending.begin.clone();
        wire.extend(&pending.end);
        wire.extend(b"prompt");
        navigation.request("'/unsent'".into());
        navigation.cancel_queued();
        assert_eq!(navigation.filter(&wire), b"\r\x1b[2Kprompt");
        assert!(navigation.command().is_none());
    }
    #[test]
    fn directory_echo_filter_preserves_errors_split_markers_and_user_output() {
        let mut navigation = Navigation::default();
        navigation.request("'/home/AR/with space'".into());
        let command = navigation.command().unwrap();
        let pending = navigation.pending.as_ref().unwrap();
        let begin = pending.begin.clone();
        let end = pending.end.clone();
        let mut wire = b"BACKGROUND\r\n".to_vec();
        wire.extend(command.clone());
        wire.extend(&begin);
        wire.extend(b"cd: permission denied\r\n");
        wire.extend(&end);
        wire.extend(b"[AR@fic ~]$ user-visible");
        let mut result = vec![];
        for byte in wire {
            result.extend(navigation.filter(&[byte]));
        }
        assert_eq!(
            result,
            b"\r\x1b[2KBACKGROUND\r\ncd: permission denied\r\n[AR@fic ~]$ user-visible"
        );
        assert_eq!(navigation.filter(b"normal\x00output"), b"normal\x00output");
        navigation.request("'/home'".into());
        navigation.command().unwrap();
        navigation.filter(b"unexpected program output");
        assert_eq!(
            navigation.expire(Instant::now() + Duration::from_secs(3)),
            b"unexpected program output"
        );
        assert!(navigation.pending.is_none());
        navigation.request("'/first'".into());
        navigation.request("'/last'".into());
        assert!(String::from_utf8(navigation.command().unwrap())
            .unwrap()
            .contains("'/last'"));
    }
}
