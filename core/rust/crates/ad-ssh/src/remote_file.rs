//! File workspace operations over the existing SFTP transport.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::Notify;

#[derive(Default)]
pub struct FileProgress {
    pub transferred: AtomicU64,
    pub total: AtomicU64,
    cancelled: AtomicBool,
    pub(super) committing: AtomicBool,
    creating_stage: AtomicBool,
    wake: Notify,
    stages: parking_lot::Mutex<Vec<String>>,
}
impl FileProgress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.wake.notify_one();
    }
    pub async fn cancelled(&self) {
        loop {
            let wake = self.wake.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            wake.await;
        }
    }
    pub(super) fn check(&self) -> CoreResult<()> {
        if self.cancelled.load(Ordering::Acquire) && !self.committing.load(Ordering::Acquire) {
            Err(sftp_error("operation cancelled"))
        } else {
            Ok(())
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileMetadata {
    pub path: String,
    pub canonical_path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub modified: Option<u32>,
    pub permissions: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub revision: Option<String>,
    pub atomic_replace: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteFileRequest {
    pub operation: String,
    pub path: String,
    pub local_path: Option<String>,
    pub destination: Option<String>,
    pub expected_revision: Option<String>,
    pub permissions: Option<u32>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileResult {
    pub host_identity: Option<HostIdentity>,
    pub metadata: Option<RemoteFileMetadata>,
    pub entries: Option<Vec<SftpEntry>>,
}

impl SshSession {
    pub async fn file_operation(
        &self,
        request: RemoteFileRequest,
        progress: Arc<FileProgress>,
    ) -> CoreResult<RemoteFileResult> {
        validate_path(&request.path)?;
        if let Some(destination) = &request.destination {
            validate_path(destination)?;
        }
        progress.check()?;
        let transport = self.bulk.as_deref().unwrap_or(self);
        let outcome = async {
            let channel = OwnedChannel(Some(transport.session.channel_open_session().await?));
            channel.request_subsystem(true, "sftp").await?;
            let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
            sftp.progress = Some(progress.clone());
            sftp.file_operation(request).await
        };
        // Cancellation closes the dedicated channel; cleanup uses a fresh channel,
        // never a protocol stream with abandoned in-flight packet replies.
        let mut work = Box::pin(tokio::time::timeout(Duration::from_secs(1800), outcome));
        let mut result = tokio::select! {
            result = &mut work => result.unwrap_or_else(|_| Err(sftp_error("file operation timed out; verify the host file before retrying"))),
            _ = progress.cancelled() => {
                if progress.committing.load(Ordering::Acquire) || progress.creating_stage.load(Ordering::Acquire) {
                    // Once rename begins, report its real result. Cancelling a
                    // submitted rename can otherwise claim a saved file failed.
                    work.as_mut().await.unwrap_or_else(|_| Err(sftp_error("save confirmation timed out; verify the host file before retrying")))
                } else { Err(sftp_error("operation cancelled")) }
            },
        };
        drop(work);
        let stages = std::mem::take(&mut *progress.stages.lock());
        if !stages.is_empty() {
            let cleanup = async {
                let channel = OwnedChannel(Some(transport.session.channel_open_session().await?));
                channel.request_subsystem(true, "sftp").await?;
                let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
                for stage in stages {
                    let _ = sftp
                        .path_request(13, &stage, "remove abandoned stage")
                        .await;
                }
                Ok::<_, CoreError>(())
            };
            let _ = timeout(Duration::from_secs(10), cleanup).await;
        }
        if let Ok(value) = &mut result {
            value.host_identity = Some(transport.host_identity.clone());
        }
        result
    }
}

pub(super) fn validate_path(path: &str) -> CoreResult<()> {
    if path.is_empty() || path.len() > 32_768 || path.contains('\0') {
        return Err(sftp_error("invalid file path"));
    }
    Ok(())
}

impl<S: AsyncRead + AsyncWrite + Unpin> SftpProtocol<S> {
    pub(super) fn update_progress(&self, transferred: u64, total: u64) -> CoreResult<()> {
        if let Some(progress) = &self.progress {
            progress.check()?;
            progress.transferred.store(transferred, Ordering::Relaxed);
            progress.total.store(total, Ordering::Relaxed);
        }
        Ok(())
    }
    pub(super) async fn path_request(
        &mut self,
        packet: u8,
        path: &str,
        operation: &str,
    ) -> CoreResult<()> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        let (kind, response) = self.request(packet, payload).await?;
        self.expect_ok(kind, &response, operation)
    }
    pub(super) async fn attrs(
        &mut self,
        path: &str,
        follow: bool,
    ) -> CoreResult<Option<SftpAttrs>> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        let (kind, response) = self
            .request(if follow { SSH_FXP_STAT } else { 7 }, payload)
            .await?;
        if kind == SSH_FXP_STATUS {
            let status = parse_status(&response)?;
            if status.code == 2 {
                return Ok(None);
            }
            return Err(self.response_error(kind, &response, "stat")?);
        }
        if kind != SSH_FXP_ATTRS {
            return Err(sftp_error("invalid stat response"));
        }
        Ok(Some(PacketCursor::new(&response).read_attrs()?))
    }
    pub(super) async fn realpath(&mut self, path: &str) -> CoreResult<String> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        let (kind, response) = self.request(16, payload).await?;
        if kind != SSH_FXP_NAME {
            return Err(self.response_error(kind, &response, "resolve path")?);
        }
        let mut cursor = PacketCursor::new(&response);
        if cursor.read_u32()? != 1 {
            return Err(sftp_error("ambiguous canonical path"));
        }
        String::from_utf8(cursor.read_string()?.to_vec())
            .map_err(|_| sftp_error("path is not UTF-8"))
    }
    pub(super) async fn metadata(
        &mut self,
        path: &str,
        fingerprint: bool,
    ) -> CoreResult<RemoteFileMetadata> {
        let attrs = self
            .attrs(path, false)
            .await?
            .ok_or_else(|| sftp_error("file no longer exists"))?;
        let canonical_path = self.realpath(path).await?;
        let kind = attrs.kind("");
        let revision = if fingerprint && kind == "file" {
            Some(self.fingerprint(&canonical_path).await?)
        } else {
            None
        };
        Ok(RemoteFileMetadata {
            path: path.to_string(),
            canonical_path,
            kind,
            size: attrs.size,
            modified: attrs.modified,
            permissions: attrs.permissions,
            uid: attrs.uid,
            gid: attrs.gid,
            revision,
            atomic_replace: self
                .extensions
                .get("posix-rename@openssh.com")
                .is_some_and(|v| v == "1"),
        })
    }
    pub(super) async fn fingerprint(&mut self, path: &str) -> CoreResult<String> {
        let handle = self.open_file(path, SSH_FXF_READ).await?;
        let mut context = ring::digest::Context::new(&ring::digest::SHA256);
        let mut offset = 0_u64;
        loop {
            if let Some(progress) = &self.progress {
                progress.check()?;
            }
            let mut payload = Vec::new();
            put_string(&mut payload, &handle)?;
            put_u64(&mut payload, offset);
            put_u32(&mut payload, SFTP_READ_CHUNK_SIZE as u32);
            let (kind, response) = self.request(SSH_FXP_READ, payload).await?;
            if kind == SSH_FXP_STATUS && parse_status(&response)?.code == SSH_FX_EOF {
                break;
            }
            if kind != SSH_FXP_DATA {
                return Err(self.response_error(kind, &response, "read revision")?);
            }
            let mut cursor = PacketCursor::new(&response);
            let data = cursor.read_string()?;
            if data.is_empty() {
                return Err(sftp_error("empty read before EOF"));
            }
            context.update(data);
            offset += data.len() as u64;
        }
        self.close_handle(&handle).await?;
        Ok(context
            .finish()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    pub(super) async fn chmod(&mut self, path: &str, mode: u32) -> CoreResult<()> {
        if mode > 0o7777 {
            return Err(sftp_error("invalid permissions"));
        }
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        put_u32(&mut payload, 4);
        put_u32(&mut payload, mode);
        let (kind, response) = self.request(9, payload).await?;
        self.expect_ok(kind, &response, "set permissions")
    }
    pub(super) async fn rename(
        &mut self,
        source: &str,
        destination: &str,
        replace: bool,
    ) -> CoreResult<()> {
        let mut payload = Vec::new();
        let packet = if replace {
            if self
                .extensions
                .get("posix-rename@openssh.com")
                .is_none_or(|v| v != "1")
            {
                return Err(sftp_error(
                    "this server cannot safely replace files atomically; save a copy instead",
                ));
            }
            put_string(&mut payload, b"posix-rename@openssh.com")?;
            200
        } else {
            18
        };
        put_string(&mut payload, source.as_bytes())?;
        put_string(&mut payload, destination.as_bytes())?;
        let (kind, response) = self.request(packet, payload).await?;
        self.expect_ok(kind, &response, "rename")
    }
    async fn mkdir(&mut self, path: &str) -> CoreResult<()> {
        if let Some(attrs) = self.attrs(path, false).await? {
            if attrs.kind("") == "directory" {
                return Ok(());
            }
            return Err(sftp_error("destination exists and is not a directory"));
        }
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        put_u32(&mut payload, 4);
        put_u32(&mut payload, 0o700);
        let (kind, response) = self.request(14, payload).await?;
        self.expect_ok(kind, &response, "create directory")
    }
    async fn remove_tree(&mut self, path: &str, recursive: bool) -> CoreResult<()> {
        let canonical = self.realpath(path).await?;
        if canonical == "/" || path == "." || path == ".." {
            return Err(sftp_error("refusing to delete a root directory"));
        }
        let mut stack = vec![(path.to_string(), false, 0_usize)];
        let mut visited = 0_usize;
        while let Some((current, children_done, depth)) = stack.pop() {
            visited += 1;
            if visited > 100_000 || depth > 128 {
                return Err(sftp_error("directory traversal limit exceeded"));
            }
            if let Some(progress) = &self.progress {
                progress.check()?;
            }
            let attrs = self
                .attrs(&current, false)
                .await?
                .ok_or_else(|| sftp_error("file no longer exists"))?;
            if attrs.kind("") != "directory" {
                self.path_request(13, &current, "remove file").await?;
                continue;
            }
            if recursive && !children_done {
                stack.push((current.clone(), true, depth));
                for entry in self.read_dir(&current).await? {
                    stack.push((entry.path, false, depth + 1));
                }
            } else {
                self.path_request(15, &current, "remove directory").await?;
            }
        }
        Ok(())
    }
    async fn staged_save(
        &mut self,
        request: &RemoteFileRequest,
        create: bool,
    ) -> CoreResult<RemoteFileMetadata> {
        validate_path(&request.path)?;
        let original = if create {
            if self.attrs(&request.path, false).await?.is_some() {
                return Err(sftp_error("destination already exists"));
            }
            None
        } else {
            let original = self.metadata(&request.path, true).await?;
            if original.kind != "file" {
                return Err(sftp_error(
                    "save requires a regular file; open the explicit link target first",
                ));
            }
            if !original.atomic_replace {
                return Err(sftp_error(
                    "server does not support safe atomic replacement; use Save a copy",
                ));
            }
            if !request.overwrite && original.revision != request.expected_revision {
                return Err(sftp_error(
                    "CONFLICT: the host file changed since it was opened",
                ));
            }
            Some(original)
        };
        let (parent, name) = request
            .path
            .rsplit_once('/')
            .unwrap_or((".", &request.path));
        if name.is_empty() || name == "." || name == ".." {
            return Err(sftp_error("invalid destination filename"));
        }
        let parent = self
            .realpath(if parent.is_empty() { "/" } else { parent })
            .await?;
        let destination = original
            .as_ref()
            .map(|v| v.canonical_path.clone())
            .unwrap_or_else(|| format!("{}/{name}", parent.trim_end_matches('/')));
        let temporary = format!(
            "{}/.aetherdesk-save-{}",
            parent.trim_end_matches('/'),
            uuid::Uuid::new_v4()
        );
        let local = request
            .local_path
            .as_deref()
            .ok_or_else(|| sftp_error("localPath is required"))?;
        let mut local_file = tokio::fs::File::open(local).await?;
        let local_attrs = local_file.metadata().await?;
        if !local_attrs.is_file() {
            return Err(sftp_error("working copy is not a regular file"));
        }
        let total = local_attrs.len();
        let mut open = Vec::new();
        put_string(&mut open, temporary.as_bytes())?;
        put_u32(&mut open, SSH_FXF_WRITE | SSH_FXF_CREAT | 0x20);
        put_u32(&mut open, 4);
        put_u32(&mut open, 0o600);
        if let Some(progress) = &self.progress {
            progress.creating_stage.store(true, Ordering::Release);
        }
        let (kind, response) = timeout(Duration::from_secs(30), self.request(SSH_FXP_OPEN, open))
            .await
            .map_err(|_| sftp_error("staging-file creation timed out"))??;
        if kind != SSH_FXP_HANDLE {
            return Err(self.response_error(kind, &response, "stage file")?);
        }
        let handle = PacketCursor::new(&response).read_string()?.to_vec();
        if let Some(progress) = &self.progress {
            progress.stages.lock().push(temporary.clone());
            progress.creating_stage.store(false, Ordering::Release);
        }
        let result = async {
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let mut buffer = vec![0; SFTP_WRITE_CHUNK_SIZE];
            let mut offset = 0;
            self.update_progress(0, total)?;
            loop {
                let n = local_file.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
                let mut payload = Vec::new();
                put_string(&mut payload, &handle)?;
                put_u64(&mut payload, offset);
                put_string(&mut payload, &buffer[..n])?;
                let (kind, response) = self.request(SSH_FXP_WRITE, payload).await?;
                self.expect_ok(kind, &response, "write staged file")?;
                offset += n as u64;
                self.update_progress(offset, total)?;
            }
            // Keep the exclusive handle open throughout the upload. Reopening a
            // pathname could follow a link introduced after exclusive creation.
            self.close_handle(&handle).await?;
            let expected: String = hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            local_file.seek(SeekFrom::Start(0)).await?;
            let mut local_hash = ring::digest::Context::new(&ring::digest::SHA256);
            loop {
                let n = local_file.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                local_hash.update(&buffer[..n]);
                if let Some(progress) = &self.progress {
                    progress.check()?;
                }
            }
            let local_revision: String = local_hash
                .finish()
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if self.fingerprint(&temporary).await? != expected
                || local_revision != expected
                || local_file.metadata().await?.len() != total
            {
                return Err(sftp_error(
                    "working copy changed during upload; retry after saving finishes",
                ));
            }
            if let Some(original) = &original {
                let current = self.metadata(&destination, true).await?;
                if current.kind != "file"
                    || current.canonical_path != original.canonical_path
                    || (!request.overwrite && current.revision != original.revision)
                {
                    return Err(sftp_error("CONFLICT: the host file changed during upload"));
                }
                // Ownership must survive atomic replacement too. Refuse a save
                // that cannot preserve the original owner and group.
                if let (Some(uid), Some(gid)) = (original.uid, original.gid) {
                    let mut payload = Vec::new();
                    put_string(&mut payload, temporary.as_bytes())?;
                    put_u32(&mut payload, 2);
                    put_u32(&mut payload, uid);
                    put_u32(&mut payload, gid);
                    let (kind, response) = self.request(9, payload).await?;
                    self.expect_ok(kind, &response, "preserve file ownership")?;
                }
                if let Some(mode) = original.permissions {
                    self.chmod(&temporary, mode & 0o7777).await?;
                }
            }
            if let Some(progress) = &self.progress {
                progress.check()?;
            }
            if let Some(progress) = &self.progress {
                progress.committing.store(true, Ordering::Release);
            }
            self.rename(&temporary, &destination, !create).await?;
            if let Some(progress) = &self.progress {
                progress.stages.lock().retain(|p| p != &temporary);
            }
            let committed = self.metadata(&destination, true).await?;
            if committed.revision.as_deref() != Some(expected.as_str()) {
                return Err(sftp_error(
                    "CONFLICT: another writer changed the host file immediately after saving",
                ));
            }
            Ok(committed)
        }
        .await;
        if result.is_err() {
            // Synchronous protocol requests leave a valid stream on normal errors.
            // Interrupted operations are cleaned through the new channel above.
            let _ = self
                .path_request(13, &temporary, "remove failed stage")
                .await;
        }
        result
    }
    async fn file_operation(&mut self, request: RemoteFileRequest) -> CoreResult<RemoteFileResult> {
        let mut metadata = None;
        let mut entries = None;
        match request.operation.as_str() {
            "stat" => metadata = Some(self.metadata(&request.path, false).await?),
            "statOptional" => {
                if self.attrs(&request.path, false).await?.is_some() {
                    metadata = Some(self.metadata(&request.path, false).await?);
                }
            }
            "revision" => metadata = Some(self.metadata(&request.path, true).await?),
            "list" => {
                let directory = self.metadata(&request.path, false).await?;
                if directory.kind != "directory" {
                    return Err(sftp_error("Choose a directory"));
                }
                entries = Some(self.read_dir(&directory.canonical_path).await?);
                metadata = Some(directory);
            }
            "download" => {
                let before = self.metadata(&request.path, true).await?;
                if before.kind != "file" {
                    return Err(sftp_error("open requires a regular file"));
                }
                self.download(
                    &before.canonical_path,
                    request
                        .local_path
                        .as_deref()
                        .ok_or_else(|| sftp_error("localPath is required"))?,
                    false,
                )
                .await?;
                let after = self.metadata(&before.canonical_path, true).await?;
                if before.revision != after.revision {
                    return Err(sftp_error("host file changed while opening; retry"));
                }
                let mut local_file = tokio::fs::File::open(
                    request.local_path.as_deref().expect("validated localPath"),
                )
                .await?;
                let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
                let mut buffer = vec![0; SFTP_READ_CHUNK_SIZE];
                loop {
                    let n = local_file.read(&mut buffer).await?;
                    if n == 0 {
                        break;
                    }
                    digest.update(&buffer[..n]);
                    if let Some(progress) = &self.progress {
                        progress.check()?;
                    }
                }
                let downloaded: String = digest
                    .finish()
                    .as_ref()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                if Some(downloaded) != after.revision {
                    return Err(sftp_error("host file changed while opening; retry"));
                }
                metadata = Some(after);
            }
            "save" => metadata = Some(self.staged_save(&request, false).await?),
            "createCopy" => metadata = Some(self.staged_save(&request, true).await?),
            "upload" => {
                let exists = self.attrs(&request.path, false).await?.is_some();
                if exists && !request.overwrite {
                    return Err(sftp_error(
                        "destination already exists; confirm replacement first",
                    ));
                }
                metadata = Some(self.staged_save(&request, !exists).await?);
            }
            "rename" => {
                self.rename(
                    &request.path,
                    request
                        .destination
                        .as_deref()
                        .ok_or_else(|| sftp_error("destination is required"))?,
                    false,
                )
                .await?
            }
            "mkdir" => self.mkdir(&request.path).await?,
            "delete" => self.remove_tree(&request.path, request.recursive).await?,
            "chmod" => {
                self.chmod(
                    &request.path,
                    request
                        .permissions
                        .ok_or_else(|| sftp_error("permissions are required"))?,
                )
                .await?
            }
            _ => return Err(sftp_error("unknown file operation")),
        }
        Ok(RemoteFileResult {
            host_identity: None,
            metadata,
            entries,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    async fn server() -> (
        SftpProtocol<tokio::io::DuplexStream>,
        tokio::process::Child,
        tokio::task::JoinHandle<()>,
    ) {
        let mut child = tokio::process::Command::new(sftp_test_server_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (client, bridge) = tokio::io::duplex(1024 * 1024);
        let (mut read, mut write) = tokio::io::split(bridge);
        let proxy = tokio::spawn(async move {
            let _ = tokio::join!(
                tokio::io::copy(&mut read, &mut input),
                tokio::io::copy(&mut output, &mut write)
            );
        });
        (SftpProtocol::connect(client).await.unwrap(), child, proxy)
    }
    fn request(operation: &str, path: &str) -> RemoteFileRequest {
        RemoteFileRequest {
            operation: operation.into(),
            path: path.into(),
            local_path: None,
            destination: None,
            expected_revision: None,
            permissions: None,
            overwrite: false,
            recursive: false,
        }
    }
    #[tokio::test]
    async fn atomic_save_preserves_mode_and_detects_same_size_conflicts() {
        use std::os::unix::fs::PermissionsExt;
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("host spaced ñ.txt");
        let local = dir.path().join("work.txt");
        tokio::fs::write(&remote, b"original").await.unwrap();
        tokio::fs::write(&local, b"new text").await.unwrap();
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o751)).unwrap();
        let path = remote.to_str().unwrap();
        let before = sftp.metadata(path, true).await.unwrap();
        assert!(before.atomic_replace);
        let mut save = request("save", path);
        save.local_path = Some(local.to_str().unwrap().into());
        save.expected_revision = before.revision;
        let result = sftp.file_operation(save.clone()).await.unwrap();
        assert!(result.metadata.unwrap().revision.is_some());
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"new text");
        assert_eq!(
            std::fs::metadata(&remote).unwrap().permissions().mode() & 0o7777,
            0o751
        );
        tokio::fs::write(&remote, b"changed!").await.unwrap();
        let conflict = sftp.file_operation(save).await.unwrap_err();
        assert!(conflict.to_string().contains("CONFLICT"));
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"changed!");
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".aetherdesk-save-")));
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }
    #[tokio::test]
    async fn unsupported_atomic_replace_never_truncates_the_host_file() {
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("host");
        tokio::fs::write(&remote, b"keep me").await.unwrap();
        sftp.extensions.clear();
        assert!(sftp
            .file_operation(request("save", remote.to_str().unwrap()))
            .await
            .is_err());
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"keep me");
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }
    #[tokio::test]
    async fn delete_folder_does_not_follow_symlinks_and_file_actions_work() {
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        let folder = dir.path().join("folder");
        tokio::fs::write(&outside, b"keep").await.unwrap();
        sftp.mkdir(folder.to_str().unwrap()).await.unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("link")).unwrap();
        let mut remove = request("delete", folder.to_str().unwrap());
        remove.recursive = true;
        sftp.file_operation(remove).await.unwrap();
        assert_eq!(tokio::fs::read(&outside).await.unwrap(), b"keep");
        assert!(!folder.exists());
        assert!(sftp.file_operation(request("delete", "/")).await.is_err());
        let destination = dir.path().join("renamed");
        sftp.rename(
            outside.to_str().unwrap(),
            destination.to_str().unwrap(),
            false,
        )
        .await
        .unwrap();
        assert!(destination.exists());
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn create_copy_and_upload_never_truncate_an_existing_destination() {
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("copy 'ñ.txt");
        let local = dir.path().join("local.txt");
        tokio::fs::write(&local, b"initial").await.unwrap();
        let mut copy = request("createCopy", remote.to_str().unwrap());
        copy.local_path = Some(local.to_str().unwrap().into());
        assert_eq!(
            sftp.file_operation(copy.clone())
                .await
                .unwrap()
                .metadata
                .unwrap()
                .kind,
            "file"
        );
        tokio::fs::write(&local, b"replacement").await.unwrap();
        assert!(sftp.file_operation(copy.clone()).await.is_err());
        copy.operation = "upload".into();
        assert!(sftp.file_operation(copy.clone()).await.is_err());
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"initial");
        copy.overwrite = true;
        sftp.file_operation(copy).await.unwrap();
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"replacement");
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_upload_cleans_its_stage_and_preserves_original() {
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("original.txt");
        let local = dir.path().join("large.txt");
        tokio::fs::write(&remote, b"keep me").await.unwrap();
        tokio::fs::write(&local, vec![b'x'; 8 * 1024 * 1024])
            .await
            .unwrap();
        let progress = Arc::new(FileProgress::default());
        sftp.progress = Some(progress.clone());
        let mut save = request("save", remote.to_str().unwrap());
        save.local_path = Some(local.to_str().unwrap().into());
        save.expected_revision = sftp
            .metadata(remote.to_str().unwrap(), true)
            .await
            .unwrap()
            .revision;
        let cancel = tokio::spawn(async move {
            while progress.transferred.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
            progress.cancel();
        });
        assert!(sftp
            .file_operation(save)
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        cancel.await.unwrap();
        assert_eq!(tokio::fs::read(&remote).await.unwrap(), b"keep me");
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".aetherdesk-save-")));
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn symlinks_and_special_files_are_not_replaced_as_regular_files() {
        let (mut sftp, mut child, proxy) = server().await;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        tokio::fs::write(&target, b"untouched").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let metadata = sftp.metadata(link.to_str().unwrap(), true).await.unwrap();
        assert_eq!(metadata.kind, "symlink");
        assert_eq!(metadata.canonical_path, target.to_str().unwrap());
        let mut save = request("save", link.to_str().unwrap());
        save.overwrite = true;
        assert!(sftp.file_operation(save).await.is_err());
        assert_eq!(tokio::fs::read(target).await.unwrap(), b"untouched");
        assert_eq!(
            SftpAttrs {
                permissions: Some(0o010600),
                ..Default::default()
            }
            .kind(""),
            "other"
        );
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_before_transfer_is_observed() {
        let progress = FileProgress::default();
        progress.cancel();
        assert!(progress.check().is_err());
        tokio::time::timeout(Duration::from_millis(20), progress.cancelled())
            .await
            .unwrap();
    }
}
