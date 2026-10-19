//! Durable local copies, tied to an authenticated host/account and revision.
use ad_ssh::{RemoteFileMetadata, SshSession};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub id: Uuid,
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub account: String,
    pub metadata: RemoteFileMetadata,
    pub filename: String,
}
pub fn identity(session: &SshSession) -> String {
    let h = session.host_identity();
    let text = format!(
        "{}\0{}\0{}\0{}",
        h.host, h.port, h.username, h.key_fingerprint
    );
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn root() -> Result<PathBuf, String> {
    let dir = crate::storage::root()?.join("working-copies");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
impl Record {
    pub fn matches(&self, session: &SshSession) -> bool {
        self.account == identity(session) && !session.is_closed()
    }
    pub fn path(&self) -> Result<PathBuf, String> {
        if self.id.is_nil()
            || self.filename != ad_desktop_model::files::safe_local_name(&self.filename)
        {
            return Err("Invalid working-copy record".into());
        }
        let root = root()?.join(self.id.to_string());
        let meta = std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err("Recovery folder is not an owned directory".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("Recovery folder is a junction or reparse point".into());
            }
        }
        let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if !root.starts_with(
            std::fs::canonicalize(super::working_copies::root()?).map_err(|e| e.to_string())?,
        ) {
            return Err("Working copy escaped its recovery folder".into());
        }
        let path = root.join(&self.filename);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("Working copy is not a regular file".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("Recovery does not follow junctions or links".into());
            }
        }
        Ok(path)
    }
    pub fn baseline(&self) -> Result<Vec<u8>, String> {
        self.read_internal(".baseline")?
            .ok_or_else(|| "Recovery baseline is missing".into())
    }
    pub fn read_internal(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        if !matches!(name, ".baseline" | ".draft") {
            return Err("Invalid recovery component".into());
        }
        let path = self.path()?.parent().unwrap().join(name);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        require_regular(&meta)?;
        if meta.len() > ad_desktop_model::files::MAX_EDITABLE_BYTES as u64 {
            return Err("Recovery component is too large".into());
        }
        std::fs::read(path).map(Some).map_err(|e| e.to_string())
    }
    pub fn save(&self) -> Result<(), String> {
        let path = self.path()?.parent().unwrap().join("record.json");
        atomic(
            &path,
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }
    pub fn advance(
        &mut self,
        metadata: RemoteFileMetadata,
        bytes: Option<&[u8]>,
    ) -> Result<(), String> {
        self.advance_baseline(metadata, bytes)?;
        if let Some(bytes) = bytes {
            atomic(&self.path()?, bytes)?;
        }
        Ok(())
    }
    /// External programs may edit again during upload. Never replace their file.
    pub fn advance_baseline(
        &mut self,
        metadata: RemoteFileMetadata,
        bytes: Option<&[u8]>,
    ) -> Result<(), String> {
        self.metadata = metadata;
        let path = self.path()?;
        if let Some(bytes) = bytes {
            atomic(&path.parent().unwrap().join(".baseline"), bytes)?;
        }
        self.save()
    }
}
fn require_regular(meta: &std::fs::Metadata) -> Result<(), String> {
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err("Recovery does not follow links".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("Recovery does not follow reparse points".into());
        }
    }
    Ok(())
}
pub fn create(
    session: &SshSession,
    metadata: RemoteFileMetadata,
    path: &Path,
    bytes: &[u8],
) -> Result<Record, String> {
    let h = session.host_identity();
    let id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .and_then(|n| Uuid::parse_str(n).ok())
        .ok_or("Invalid recovery directory")?;
    let record = Record {
        id,
        host: h.host,
        port: h.port,
        fingerprint: h.key_fingerprint,
        account: identity(session),
        metadata,
        filename: path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid working filename")?
            .into(),
    };
    atomic(&path.parent().unwrap().join(".baseline"), bytes)?;
    record.save()?;
    Ok(record)
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut stage = tempfile::NamedTempFile::new_in(path.parent().ok_or("No working-copy parent")?)
        .map_err(|e| e.to_string())?;
    stage.write_all(bytes).map_err(|e| e.to_string())?;
    stage.as_file().sync_all().map_err(|e| e.to_string())?;
    stage.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}
pub fn list() -> Result<Vec<Record>, String> {
    let mut records = vec![];
    for entry in std::fs::read_dir(root()?).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let path = entry.path().join("record.json");
        if !path.is_file() {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        require_regular(&meta)?;
        if meta.len() > 65536 {
            return Err("Oversized working-copy record preserved".into());
        }
        let record: Record =
            serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|_| "Damaged working-copy record preserved")?;
        if entry.file_name().to_str() != Some(record.id.to_string().as_str()) {
            return Err("Working-copy identity does not match its folder".into());
        }
        record.path()?;
        records.push(record);
    }
    records.sort_by_key(|r| r.metadata.canonical_path.clone());
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_recovery_write_preserves_complete_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        atomic(&file, b"first").unwrap();
        atomic(&file, b"second").unwrap();
        assert_eq!(std::fs::read(file).unwrap(), b"second");
    }
}
