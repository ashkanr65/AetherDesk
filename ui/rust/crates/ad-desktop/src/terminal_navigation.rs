//! Suppress only the echo of an application-owned directory command. Real
//! command output, shell errors and all bytes after the end marker remain.
use std::time::{Duration, Instant};
pub(crate) struct Navigation {
    queued: Option<String>,
    pending: Option<Pending>,
    prompt: Vec<u8>,
    prompt_fragment: Vec<u8>,
    token: String,
    ready: bool,
    enabled: bool,
}
struct Pending {
    begin: Vec<u8>,
    end: Vec<u8>,
    bytes: Vec<u8>,
    echo: Vec<u8>,
    nonce: Vec<u8>,
    started: Instant,
}
impl Default for Navigation {
    fn default() -> Self {
        let token = uuid::Uuid::new_v4().simple().to_string();
        Self {
            prompt: format!("\x1b]777;{token};prompt\x07").into_bytes(),
            token,
            queued: None,
            pending: None,
            prompt_fragment: Vec::new(),
            ready: false,
            enabled: true,
        }
    }
}
fn position(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|b| b == needle)
}
fn echo_start(pending: &Pending, before: usize) -> Option<usize> {
    let bytes = &pending.bytes[..before];
    position(bytes, &pending.echo).or_else(|| {
        // Readline may wrap a long quoted path. A matching per-request nonce
        // still distinguishes our echo from ordinary program output.
        let token = position(bytes, &pending.nonce)?;
        let prefix = b" __aetherdesk_cd ";
        bytes[..token]
            .windows(prefix.len())
            .rposition(|part| part == prefix)
    })
}
impl Navigation {
    pub(crate) fn bootstrap(&self, integration: &str) -> String {
        format!("__aetherdesk_prompt_token='{}'; {integration}", self.token)
    }
    pub(crate) fn user_input(&mut self) {
        // A typed line, paste, foreground program or alternate screen owns stdin
        // until Bash reports a new prompt. Never append navigation to that input.
        self.ready = false;
        self.queued = None;
    }
    pub(crate) fn request(&mut self, quoted_path: String) {
        if self.enabled && !quoted_path.chars().any(char::is_control) {
            self.queued = Some(quoted_path);
        }
    }
    pub(crate) fn cancel_queued(&mut self) {
        self.queued = None;
    }
    pub(crate) fn command(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_some() || !self.enabled || !self.ready {
            return None;
        }
        let path = self.queued.take()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let echo = format!(" __aetherdesk_cd {path} '{token}'").into_bytes();
        self.pending = Some(Pending {
            begin: format!("\x1b]777;{token};begin\x07").into_bytes(),
            end: format!("\x1b]777;{token};end\x07").into_bytes(),
            bytes: vec![],
            echo: echo.clone(),
            nonce: token.into_bytes(),
            started: Instant::now(),
        });
        self.ready = false;
        Some([echo, vec![b'\r']].concat())
    }
    pub(crate) fn filter(&mut self, bytes: &[u8]) -> Vec<u8> {
        let bytes = self.prompt_output(bytes);
        let Some(pending) = &mut self.pending else {
            return bytes;
        };
        pending.bytes.extend_from_slice(&bytes);
        if let Some(begin) = position(&pending.bytes, &pending.begin) {
            let content = begin + pending.begin.len();
            if let Some(end) = position(&pending.bytes[content..], &pending.end) {
                let end = content + end;
                let mut result = b"\r\x1b[2K".to_vec(); // replace the existing prompt
                if let Some(echo) = echo_start(pending, begin) {
                    result.extend_from_slice(&pending.bytes[..echo]);
                } else {
                    // Echo may be disabled. Preserve all real output before the marker.
                    result.extend_from_slice(&pending.bytes[..begin]);
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
            // An unexpected shell must never hide output or accept repeated
            // injected commands. Stop synchronization until this tab reconnects.
            self.enabled = false;
            self.ready = false;
            self.queued = None;
            let mut pending = self.pending.take().unwrap();
            if let Some(begin) = position(&pending.bytes, &pending.begin) {
                let start = echo_start(&pending, begin).unwrap_or(begin);
                pending.bytes.drain(start..begin + pending.begin.len());
            } else if let Some(start) = position(&pending.bytes, &pending.echo) {
                pending.bytes.drain(start..start + pending.echo.len());
            }
            return pending.bytes;
        }
        vec![]
    }
    fn prompt_output(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut visible = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            self.prompt_fragment.push(byte);
            while !self.prompt.starts_with(&self.prompt_fragment) {
                visible.push(self.prompt_fragment.remove(0));
            }
            if self.prompt_fragment == self.prompt {
                self.prompt_fragment.clear();
                self.ready = self.enabled;
            }
        }
        visible
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn prompt(navigation: &mut Navigation) {
        assert!(navigation.filter(&navigation.prompt.clone()).is_empty());
    }
    #[test]
    fn canceling_folder_sync_keeps_inflight_echo_filter_and_drops_unsent_changes() {
        let mut navigation = Navigation::default();
        prompt(&mut navigation);
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
        prompt(&mut navigation);
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
        prompt(&mut navigation);
        navigation.request("'/home'".into());
        navigation.command().unwrap();
        navigation.filter(b"unexpected program output");
        assert_eq!(
            navigation.expire(Instant::now() + Duration::from_secs(3)),
            b"unexpected program output"
        );
        assert!(navigation.pending.is_none());
        prompt(&mut navigation);
        navigation.request("'/last'".into());
        assert!(
            navigation.command().is_none(),
            "an unconfirmed helper must stop further injection"
        );
    }
    #[test]
    fn folder_changes_wait_for_a_private_prompt_and_never_enter_running_or_typed_commands() {
        let mut navigation = Navigation::default();
        navigation.request("'/before-setup'".into());
        assert!(navigation.command().is_none());
        assert_eq!(navigation.filter(b"[axr@fic ~]$ "), b"[axr@fic ~]$ ");
        assert!(
            navigation.command().is_none(),
            "visible prompt text is not readiness proof"
        );
        let marker = navigation.prompt.clone();
        for byte in marker {
            assert!(navigation.filter(&[byte]).is_empty());
        }
        navigation.user_input(); // cp, rm, cat, pasted text or a partially typed line
        navigation.request("'/first'".into());
        navigation.request("'/last'".into());
        assert!(navigation.command().is_none());
        assert_eq!(
            navigation.filter(b"cp: permission denied\r\n"),
            b"cp: permission denied\r\n"
        );
        assert!(navigation.command().is_none());
        prompt(&mut navigation);
        assert!(String::from_utf8(navigation.command().unwrap())
            .unwrap()
            .contains("'/last'"));
        assert!(navigation.command().is_none());
    }
    #[test]
    fn user_input_cancels_unsent_sync_and_unconfirmed_echo_cannot_trigger_retries() {
        let mut navigation = Navigation::default();
        prompt(&mut navigation);
        navigation.request("'/old-choice'".into());
        navigation.user_input();
        prompt(&mut navigation);
        assert!(navigation.command().is_none());
        navigation.request("'/new-choice'".into());
        let command = navigation.command().unwrap();
        let mut echo = command[..command.len() - 1].to_vec();
        echo.extend_from_slice(b"\r\nREAL OUTPUT\r\n");
        navigation.filter(&echo);
        assert_eq!(
            navigation.expire(Instant::now() + Duration::from_secs(3)),
            b"\r\nREAL OUTPUT\r\n"
        );
        prompt(&mut navigation);
        navigation.request("'/no-retry'".into());
        assert!(navigation.command().is_none());
    }
    #[test]
    fn wrapped_navigation_echo_is_owned_by_its_nonce_and_real_errors_remain_visible() {
        let mut navigation = Navigation::default();
        prompt(&mut navigation);
        navigation.request("'/a/long/directory/name'".into());
        let command = navigation.command().unwrap();
        let pending = navigation.pending.as_ref().unwrap();
        let mut wire = b"BACKGROUND\r\n".to_vec();
        wire.extend_from_slice(&command[..20]);
        wire.extend_from_slice(b"\r\n");
        wire.extend_from_slice(&command[20..command.len() - 1]);
        wire.extend_from_slice(&pending.begin);
        wire.extend_from_slice(b"cd: permission denied\r\n");
        wire.extend_from_slice(&pending.end);
        assert_eq!(
            navigation.filter(&wire),
            b"\r\x1b[2KBACKGROUND\r\ncd: permission denied\r\n"
        );
    }
}
