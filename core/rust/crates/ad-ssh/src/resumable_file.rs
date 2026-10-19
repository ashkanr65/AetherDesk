//! Restartable private stages. Source snapshots and partial prefixes are hashed
//! before continuation; only a complete verified file is published.
use super::remote_file::validate_path;
use super::*;
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResumableTransferFile {
    pub upload: bool,
    pub job_id: uuid::Uuid,
    pub ordinal: u32,
    pub local_path: String,
    pub remote_path: String,
    pub remote_parent: String,
    pub local_parent: String,
    pub source_revision: String,
    pub source_size: u64,
    pub destination_revision: Option<String>,
}
impl ResumableTransferFile {
    pub fn validate(&self) -> CoreResult<()> {
        if self.job_id.is_nil()
            || self.ordinal >= 100000
            || self.source_revision.len() != 64
            || !self.source_revision.bytes().all(|b| b.is_ascii_hexdigit())
            || self
                .destination_revision
                .as_ref()
                .is_some_and(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(sftp_error("Invalid transfer checkpoint"));
        }
        validate_path(&self.remote_path)?;
        validate_path(&self.remote_parent)?;
        if !self.remote_path.starts_with('/')
            || !self.remote_parent.starts_with('/')
            || self.local_path.contains('\0')
            || !Path::new(&self.local_path).is_absolute()
            || !Path::new(&self.local_parent).is_absolute()
        {
            return Err(sftp_error("Checkpoint paths must be absolute"));
        }
        let (parent, name) = self
            .remote_path
            .rsplit_once('/')
            .ok_or_else(|| sftp_error("Invalid remote filename"))?;
        if name.is_empty()
            || matches!(name, "." | "..")
            || (if parent.is_empty() { "/" } else { parent }) != self.remote_parent
        {
            return Err(sftp_error("Checkpoint parent changed"));
        }
        if Path::new(&self.local_path).parent() != Some(Path::new(&self.local_parent)) {
            return Err(sftp_error("Checkpoint local parent changed"));
        }
        Ok(())
    }
    fn stage_name(&self) -> String {
        format!(".aetherdesk-transfer-{}-{}.part", self.job_id, self.ordinal)
    }
    pub fn local_stage(&self) -> PathBuf {
        Path::new(&self.local_parent).join(self.stage_name())
    }
    fn remote_stage(&self) -> String {
        format!(
            "{}/{}",
            self.remote_parent.trim_end_matches('/'),
            self.stage_name()
        )
    }
}
impl SshSession {
    pub async fn transfer_file_resumable(
        &self,
        spec: ResumableTransferFile,
        progress: Arc<FileProgress>,
    ) -> CoreResult<()> {
        spec.validate()?;
        progress.check()?;
        let transport = self.bulk.as_deref().unwrap_or(self);
        let operation = async {
            let channel = OwnedChannel(Some(transport.session.channel_open_session().await?));
            channel.request_subsystem(true, "sftp").await?;
            let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
            sftp.progress = Some(progress.clone());
            if sftp.realpath(&spec.remote_parent).await? != spec.remote_parent {
                return Err(sftp_error("CONFLICT: remote transfer parent changed"));
            }
            if std::fs::canonicalize(&spec.local_parent)? != PathBuf::from(&spec.local_parent) {
                return Err(sftp_error("CONFLICT: local transfer parent changed"));
            }
            if spec.upload {
                sftp.resume_upload(&spec).await
            } else {
                sftp.resume_download(&spec).await
            }
        };
        let mut work = Box::pin(timeout(Duration::from_secs(1800), operation));
        let result = tokio::select! {
            result=&mut work=>result.map_err(|_|sftp_error("Transfer timed out; its checkpoint was retained"))?,
            _=progress.cancelled()=>{
                if progress.committing.load(Ordering::Acquire){timeout(Duration::from_secs(30),work.as_mut()).await.map_err(|_|sftp_error("Publish confirmation timed out; verify destination before retrying"))?.map_err(|_|sftp_error("Transfer timed out"))?}
                else{Err(sftp_error("Transfer paused; partial data retained"))}
            },
            _=self.lifetime.0.cancelled()=>Err(sftp_error("Connection closed; transfer checkpoint retained")),
        };
        drop(work);
        progress.committing.store(false, Ordering::Release);
        result
    }
    pub async fn discard_transfer_stage(&self, spec: &ResumableTransferFile) -> CoreResult<()> {
        spec.validate()?;
        if !spec.upload {
            let parent = match std::fs::canonicalize(&spec.local_parent) {
                Ok(parent) => parent,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.into()),
            };
            if parent != PathBuf::from(&spec.local_parent) {
                return Err(sftp_error("Checkpoint parent changed; cleanup refused"));
            }
            let path = spec.local_stage();
            if path.exists() {
                let file = open_local(&path, false, false)?;
                drop(file);
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }
        let transport = self.bulk.as_deref().unwrap_or(self);
        let channel = OwnedChannel(Some(transport.session.channel_open_session().await?));
        channel.request_subsystem(true, "sftp").await?;
        let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
        if sftp.attrs(&spec.remote_parent, false).await?.is_none() {
            return Ok(());
        }
        if sftp.realpath(&spec.remote_parent).await? != spec.remote_parent {
            return Err(sftp_error("Transfer parent changed; cleanup refused"));
        }
        let stage = spec.remote_stage();
        if let Some(attrs) = sftp.attrs(&stage, false).await? {
            if attrs.kind("") != "file" || sftp.realpath(&stage).await? != stage {
                return Err(sftp_error("Transfer stage is not the owned regular file"));
            }
            sftp.path_request(13, &stage, "discard private transfer stage")
                .await?;
        }
        Ok(())
    }
}
async fn local_hash(file: &mut tokio::fs::File, limit: Option<u64>) -> CoreResult<String> {
    file.seek(SeekFrom::Start(0)).await?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut remaining = limit.unwrap_or(u64::MAX);
    let mut buffer = vec![0; 65536];
    while remaining > 0 {
        let cap = buffer.len().min(remaining.min(65536) as usize);
        let n = file.read(&mut buffer[..cap]).await?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    if limit.is_some() && remaining != 0 {
        return Err(sftp_error("Checkpoint prefix is incomplete"));
    }
    Ok(hash
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn open_local(path: &Path, write: bool, create: bool) -> CoreResult<tokio::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(write).create_new(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000).mode(0o600);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(sftp_error("Transfer requires regular local files"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(sftp_error(
                "Transfer does not follow local links or junctions",
            ));
        }
    }
    Ok(tokio::fs::File::from_std(file))
}
impl<S: AsyncRead + AsyncWrite + Unpin> SftpProtocol<S> {
    async fn transfer_read(
        &mut self,
        handle: &[u8],
        offset: u64,
        size: usize,
    ) -> CoreResult<Vec<u8>> {
        let mut payload = vec![];
        put_string(&mut payload, handle)?;
        put_u64(&mut payload, offset);
        put_u32(&mut payload, size as u32);
        let (kind, response) = self.request(SSH_FXP_READ, payload).await?;
        if kind == SSH_FXP_STATUS && parse_status(&response)?.code == SSH_FX_EOF {
            return Ok(vec![]);
        }
        if kind != SSH_FXP_DATA {
            return Err(self.response_error(kind, &response, "read transfer")?);
        }
        Ok(PacketCursor::new(&response).read_string()?.to_vec())
    }
    async fn transfer_hash(&mut self, handle: &[u8], size: u64) -> CoreResult<String> {
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut offset = 0;
        while offset < size {
            if let Some(p) = &self.progress {
                p.check()?;
            }
            let bytes = self
                .transfer_read(handle, offset, (size - offset).min(65536) as usize)
                .await?;
            if bytes.is_empty() || bytes.len() as u64 > size - offset {
                return Err(sftp_error("Remote checkpoint is incomplete"));
            }
            offset += bytes.len() as u64;
            hash.update(&bytes);
        }
        Ok(hash
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    async fn optional_revision(&mut self, path: &str) -> CoreResult<Option<RemoteFileMetadata>> {
        if self.attrs(path, false).await?.is_some() {
            Ok(Some(self.metadata(path, true).await?))
        } else {
            Ok(None)
        }
    }
    async fn resume_upload(&mut self, spec: &ResumableTransferFile) -> CoreResult<()> {
        let mut local = open_local(Path::new(&spec.local_path), false, false)?;
        if local.metadata().await?.len() != spec.source_size
            || local_hash(&mut local, None).await? != spec.source_revision
        {
            return Err(sftp_error(
                "CONFLICT: upload source changed; create a new transfer",
            ));
        }
        let destination = self.optional_revision(&spec.remote_path).await?;
        if destination
            .as_ref()
            .is_some_and(|m| m.kind != "file" || m.canonical_path != spec.remote_path)
        {
            return Err(sftp_error(
                "Upload destination is not the original regular file",
            ));
        }
        if destination.as_ref().is_some_and(|m| {
            m.kind == "file" && m.revision.as_deref() == Some(spec.source_revision.as_str())
        }) {
            self.update_progress(spec.source_size, spec.source_size)?;
            return Ok(());
        }
        if destination.as_ref().and_then(|m| m.revision.as_ref())
            != spec.destination_revision.as_ref()
        {
            return Err(sftp_error("CONFLICT: upload destination changed"));
        }
        let stage = spec.remote_stage();
        let existing = self.attrs(&stage, false).await?;
        let offset = if let Some(attrs) = &existing {
            if attrs.kind("") != "file" || self.realpath(&stage).await? != stage {
                return Err(sftp_error("Transfer stage is not a regular private file"));
            }
            attrs.size.unwrap_or(0)
        } else {
            0
        };
        if offset > spec.source_size {
            return Err(sftp_error("Checkpoint exceeds source size"));
        }
        let handle = if existing.is_none() {
            let mut payload = vec![];
            put_string(&mut payload, stage.as_bytes())?;
            put_u32(
                &mut payload,
                SSH_FXF_READ | SSH_FXF_WRITE | SSH_FXF_CREAT | 0x20,
            );
            put_u32(&mut payload, 4);
            put_u32(&mut payload, 0o600);
            let (kind, response) = self.request(SSH_FXP_OPEN, payload).await?;
            if kind != SSH_FXP_HANDLE {
                return Err(self.response_error(
                    kind,
                    &response,
                    "create private transfer stage",
                )?);
            }
            PacketCursor::new(&response).read_string()?.to_vec()
        } else {
            self.open_file(&stage, SSH_FXF_READ | SSH_FXF_WRITE).await?
        };
        if self.realpath(&stage).await? != stage {
            return Err(sftp_error("Transfer stage changed while opening"));
        }
        self.chmod(&stage, 0o600).await?;
        if self.transfer_hash(&handle, offset).await?
            != local_hash(&mut local, Some(offset)).await?
        {
            return Err(sftp_error("CONFLICT: upload checkpoint differs from source; discard partial files before restarting"));
        }
        local.seek(SeekFrom::Start(offset)).await?;
        let mut offset = offset;
        let mut buffer = vec![0; SFTP_WRITE_CHUNK_SIZE];
        self.update_progress(offset, spec.source_size)?;
        loop {
            let n = local.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            let mut payload = vec![];
            put_string(&mut payload, &handle)?;
            put_u64(&mut payload, offset);
            put_string(&mut payload, &buffer[..n])?;
            let (kind, response) = self.request(SSH_FXP_WRITE, payload).await?;
            self.expect_ok(kind, &response, "write resumable transfer")?;
            offset += n as u64;
            self.update_progress(offset, spec.source_size)?;
        }
        if offset != spec.source_size
            || local_hash(&mut local, None).await? != spec.source_revision
            || self.transfer_hash(&handle, spec.source_size).await? != spec.source_revision
        {
            return Err(sftp_error("CONFLICT: transfer data changed"));
        }
        self.close_handle(&handle).await?;
        drop(local);
        let current = self.optional_revision(&spec.remote_path).await?;
        if self.realpath(&spec.remote_parent).await? != spec.remote_parent {
            return Err(sftp_error("Transfer parent changed before publishing"));
        }
        if current.as_ref().and_then(|m| m.revision.as_ref()) != spec.destination_revision.as_ref()
        {
            return Err(sftp_error("CONFLICT: destination changed while uploading"));
        }
        if let Some(original) = &destination {
            if original.kind != "file" || original.canonical_path != spec.remote_path {
                return Err(sftp_error("Destination is not the original regular file"));
            }
            if let (Some(uid), Some(gid)) = (original.uid, original.gid) {
                let mut payload = vec![];
                put_string(&mut payload, stage.as_bytes())?;
                put_u32(&mut payload, 2);
                put_u32(&mut payload, uid);
                put_u32(&mut payload, gid);
                let (kind, response) = self.request(9, payload).await?;
                self.expect_ok(kind, &response, "preserve transfer ownership")?;
            }
            if let Some(mode) = original.permissions {
                self.chmod(&stage, mode & 0o7777).await?;
            }
        } else {
            self.chmod(&stage, 0o644).await?;
        }
        if let Some(p) = &self.progress {
            p.check()?;
            p.committing.store(true, Ordering::Release);
        }
        let publish = self
            .rename(&stage, &spec.remote_path, destination.is_some())
            .await;
        if let Some(p) = &self.progress {
            p.committing.store(false, Ordering::Release);
        }
        publish?;
        if self
            .metadata(&spec.remote_path, true)
            .await?
            .revision
            .as_deref()
            != Some(spec.source_revision.as_str())
        {
            return Err(sftp_error(
                "CONFLICT: published file changed before confirmation",
            ));
        }
        Ok(())
    }
    async fn resume_download(&mut self, spec: &ResumableTransferFile) -> CoreResult<()> {
        let source = self.metadata(&spec.remote_path, true).await?;
        if source.kind != "file"
            || source.canonical_path != spec.remote_path
            || source.size != Some(spec.source_size)
            || source.revision.as_deref() != Some(spec.source_revision.as_str())
        {
            return Err(sftp_error("CONFLICT: download source changed"));
        }
        let destination = PathBuf::from(&spec.local_path);
        let current = if destination.exists() {
            let mut file = open_local(&destination, false, false)?;
            Some(local_hash(&mut file, None).await?)
        } else {
            None
        };
        if current.as_deref() == Some(spec.source_revision.as_str()) {
            self.update_progress(spec.source_size, spec.source_size)?;
            return Ok(());
        }
        if current != spec.destination_revision {
            return Err(sftp_error("CONFLICT: local destination changed"));
        }
        let stage = spec.local_stage();
        let mut local = match open_local(&stage, true, true) {
            Ok(file) => file,
            Err(CoreError::IoError(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                open_local(&stage, true, false)?
            }
            Err(error) => return Err(error),
        };
        let offset = local.metadata().await?.len();
        if offset > spec.source_size {
            return Err(sftp_error("Checkpoint exceeds remote source"));
        }
        let handle = self.open_file(&spec.remote_path, SSH_FXF_READ).await?;
        if self.transfer_hash(&handle, offset).await?
            != local_hash(&mut local, Some(offset)).await?
        {
            return Err(sftp_error(
                "CONFLICT: local checkpoint differs from source; discard it before restarting",
            ));
        }
        let mut offset = offset;
        local.seek(SeekFrom::Start(offset)).await?;
        self.update_progress(offset, spec.source_size)?;
        while offset < spec.source_size {
            let data = self
                .transfer_read(
                    &handle,
                    offset,
                    (spec.source_size - offset).min(SFTP_READ_CHUNK_SIZE as u64) as usize,
                )
                .await?;
            if data.is_empty() || data.len() as u64 > spec.source_size - offset {
                return Err(sftp_error("Download did not match its expected size"));
            }
            local.write_all(&data).await?;
            offset += data.len() as u64;
            self.update_progress(offset, spec.source_size)?;
        }
        local.flush().await?;
        local.sync_all().await?;
        if offset != spec.source_size
            || local_hash(&mut local, None).await? != spec.source_revision
            || self
                .metadata(&spec.remote_path, true)
                .await?
                .revision
                .as_deref()
                != Some(spec.source_revision.as_str())
        {
            return Err(sftp_error("CONFLICT: source changed during download"));
        }
        self.close_handle(&handle).await?;
        drop(local);
        let current = if destination.exists() {
            let mut file = open_local(&destination, false, false)?;
            Some(local_hash(&mut file, None).await?)
        } else {
            None
        };
        if current != spec.destination_revision {
            return Err(sftp_error(
                "CONFLICT: local destination changed during transfer",
            ));
        }
        if let Some(p) = &self.progress {
            p.check()?;
            p.committing.store(true, Ordering::Release);
        }
        if std::fs::canonicalize(&spec.local_parent)? != PathBuf::from(&spec.local_parent) {
            return Err(sftp_error(
                "Local transfer parent changed before publishing",
            ));
        }
        let publish = publish_local(&stage, &destination, spec.destination_revision.is_some());
        if let Some(p) = &self.progress {
            p.committing.store(false, Ordering::Release);
        }
        publish
    }
}
fn publish_local(stage: &Path, destination: &Path, replace: bool) -> CoreResult<()> {
    if !replace {
        rename_local_noclobber(stage, destination)?;
    } else {
        let permissions = std::fs::metadata(destination)?.permissions();
        std::fs::set_permissions(stage, permissions)?;
        std::fs::rename(stage, destination)?;
    }
    Ok(())
}
pub fn rename_local_noclobber(source: &Path, destination: &Path) -> CoreResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
        }
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if source[..source.len() - 1].contains(&0)
            || destination[..destination.len() - 1].contains(&0)
        {
            return Err(sftp_error("Filename contains a NUL character"));
        }
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        extern "C" {
            fn renameat2(
                old_dir: i32,
                old_path: *const std::ffi::c_char,
                new_dir: i32,
                new_path: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        let a = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| sftp_error("Invalid source path"))?;
        let b = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| sftp_error("Invalid destination path"))?;
        if unsafe { renameat2(-100, a.as_ptr(), -100, b.as_ptr(), 1) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        if source.is_dir() {
            return Err(sftp_error(
                "Atomic directory rename is unavailable on this platform",
            ));
        }
        std::fs::hard_link(source, destination)?;
        std::fs::remove_file(source)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_cannot_redirect_its_private_stage() {
        let spec = ResumableTransferFile {
            upload: true,
            job_id: uuid::Uuid::new_v4(),
            ordinal: 0,
            local_path: std::env::temp_dir().join("a").display().to_string(),
            local_parent: std::env::temp_dir().display().to_string(),
            remote_path: "/home/a".into(),
            remote_parent: "/home".into(),
            source_revision: "ab".repeat(32),
            source_size: 1,
            destination_revision: None,
        };
        assert!(spec.validate().is_ok());
        let mut bad = spec.clone();
        bad.remote_parent = "/another".into();
        assert!(bad.validate().is_err());
        bad = spec;
        bad.job_id = uuid::Uuid::nil();
        assert!(bad.validate().is_err());
    }
    #[test]
    fn publishing_a_new_download_never_overwrites_a_racing_destination() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        let stage = dir.join("stage");
        let target = dir.join("target");
        std::fs::write(&stage, b"download").unwrap();
        std::fs::write(&target, b"other").unwrap();
        assert!(publish_local(&stage, &target, false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"other");
        assert_eq!(std::fs::read(&stage).unwrap(), b"download");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    fn fixture(dir: &Path, upload: bool, bytes: &[u8]) -> ResumableTransferFile {
        let parent = std::fs::canonicalize(dir).unwrap();
        ResumableTransferFile {
            upload,
            job_id: uuid::Uuid::new_v4(),
            ordinal: 0,
            local_path: parent.join("local.txt").display().to_string(),
            local_parent: parent.display().to_string(),
            remote_path: parent.join("remote.txt").display().to_string(),
            remote_parent: parent.display().to_string(),
            source_revision: ring::digest::digest(&ring::digest::SHA256, bytes)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            source_size: bytes.len() as u64,
            destination_revision: None,
        }
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resumes_verified_upload_and_download_prefixes_on_openssh() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"complete payload including spaced utf8 \xc3\xb1";
        let (mut sftp, mut child, proxy) = server().await;
        let upload = fixture(dir.path(), true, bytes);
        std::fs::write(&upload.local_path, bytes).unwrap();
        std::fs::write(upload.remote_stage(), &bytes[..7]).unwrap();
        sftp.resume_upload(&upload).await.unwrap();
        assert_eq!(std::fs::read(&upload.remote_path).unwrap(), bytes);
        assert!(!Path::new(&upload.remote_stage()).exists());
        // Repeating after a lost publish reply is idempotent.
        sftp.resume_upload(&upload).await.unwrap();
        std::fs::remove_file(&upload.local_path).unwrap();
        let download = fixture(dir.path(), false, bytes);
        std::fs::write(download.local_stage(), &bytes[..9]).unwrap();
        sftp.resume_download(&download).await.unwrap();
        assert_eq!(std::fs::read(&download.local_path).unwrap(), bytes);
        assert!(!download.local_stage().exists());
        sftp.resume_download(&download).await.unwrap();
        let _ = child.kill().await;
        proxy.abort();
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejects_changed_sources_and_corrupt_prefixes_without_publishing() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"original source data";
        let (mut sftp, mut child, proxy) = server().await;
        let upload = fixture(dir.path(), true, bytes);
        std::fs::write(&upload.local_path, bytes).unwrap();
        std::fs::write(upload.remote_stage(), b"corrupt").unwrap();
        assert!(sftp.resume_upload(&upload).await.is_err());
        assert!(!Path::new(&upload.remote_path).exists());
        assert_eq!(std::fs::read(upload.remote_stage()).unwrap(), b"corrupt");
        std::fs::write(&upload.local_path, b"source changed").unwrap();
        assert!(sftp.resume_upload(&upload).await.is_err());
        assert_eq!(std::fs::read(upload.remote_stage()).unwrap(), b"corrupt");
        let download = fixture(dir.path(), false, bytes);
        std::fs::write(&download.remote_path, bytes).unwrap();
        std::fs::write(&download.local_path, b"user changes").unwrap();
        assert!(sftp.resume_download(&download).await.is_err());
        assert_eq!(
            std::fs::read(&download.local_path).unwrap(),
            b"user changes"
        );
        let _ = child.kill().await;
        proxy.abort();
    }
}
