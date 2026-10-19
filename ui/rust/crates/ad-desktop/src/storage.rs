use std::{io::Write, path::PathBuf};
pub fn root() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(root) = TEST_ROOT.with(|r| r.borrow().clone()) {
        return Ok(root);
    }
    let path =
        PathBuf::from(std::env::var_os("APPDATA").ok_or("Application data directory unavailable")?)
            .join("AetherDesk-Rust");
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
pub fn write_json(name: &str, value: &serde_json::Value) -> Result<(), String> {
    validate_name(name)?;
    let root = root()?;
    let mut temporary = tempfile::NamedTempFile::new_in(&root).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Settings exceed 32 MiB; remove older chat attachments before saving".into());
    }
    temporary.write_all(&bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(root.join(name))
        .map_err(|e| e.error.to_string())?;
    Ok(())
}
pub fn read_json(name: &str) -> Result<Option<serde_json::Value>, String> {
    validate_name(name)?;
    let path = root()?.join(name);
    let metadata = match std::fs::metadata(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.len() > 32 * 1024 * 1024 {
        return Err("Settings file is too large".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| format!("{name} is damaged. It has been preserved."))
}
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(['/', '\\', ':', '\0']) || matches!(name, "." | "..") {
        return Err("Invalid settings filename".into());
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_test_root<T>(root: &std::path::Path, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_ROOT.with(|r| *r.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_ROOT.with(|r| r.replace(Some(root.to_path_buf()))));
    run()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_store_preserves_corrupt_files_and_refuses_path_escape() {
        let directory = tempfile::tempdir().unwrap();
        with_test_root(directory.path(), || {
            for name in ["", ".", "..", "../sessions.json", "C:outside.json", "a\\b"] {
                assert!(write_json(name, &serde_json::json!({})).is_err());
                assert!(read_json(name).is_err());
            }
            write_json("sessions.json", &serde_json::json!([{"id":"first"}])).unwrap();
            write_json("sessions.json", &serde_json::json!([{"id":"second"}])).unwrap();
            assert_eq!(
                read_json("sessions.json").unwrap().unwrap()[0]["id"],
                "second"
            );
            std::fs::write(directory.path().join("sessions.json"), b"broken {").unwrap();
            assert!(read_json("sessions.json").is_err());
            assert_eq!(
                std::fs::read(directory.path().join("sessions.json")).unwrap(),
                b"broken {"
            );
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        });
    }
}
