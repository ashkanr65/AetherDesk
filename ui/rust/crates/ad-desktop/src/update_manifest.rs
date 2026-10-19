//! Release metadata is authenticated with a key pinned into the executable.
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{digest, signature};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

pub const MAX_METADATA: usize = 8 * 1024 * 1024;
pub const MAX_PACKAGE: u64 = 4 * 1024 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub repository: String,
    pub public_key: String,
}
pub fn config() -> Result<Config, String> {
    serde_json::from_str(include_str!("../../../../../update-config.json"))
        .map_err(|_| "Invalid embedded update configuration".into())
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub product: String,
    pub target: String,
    pub version: String,
    pub build: u64,
    pub published: i64,
    pub expires: i64,
    pub notes: String,
    pub portable: Asset,
    pub installer: Option<Asset>,
    pub files: Vec<File>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    manifest: String,
    signature: String,
}
pub fn version(value: &str, build: u64) -> Result<[u64; 4], String> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Update version must contain three numeric components".into());
    }
    Ok([
        parts[0].parse().map_err(|_| "Invalid version")?,
        parts[1].parse().map_err(|_| "Invalid version")?,
        parts[2].parse().map_err(|_| "Invalid version")?,
        build,
    ])
}
pub fn current() -> Result<[u64; 4], String> {
    let release: serde_json::Value =
        serde_json::from_str(include_str!("../../../../../release.json"))
            .map_err(|_| "Invalid embedded version")?;
    version(
        env!("CARGO_PKG_VERSION"),
        release["build"].as_u64().ok_or("Invalid build number")?,
    )
}
pub fn relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() < 220
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
                && !matches!(
                    part.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn verify(bytes: &[u8], public_key: &str, now: i64) -> Result<Manifest, String> {
    if bytes.len() > MAX_METADATA {
        return Err("Update metadata is too large".into());
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|_| "Invalid update metadata")?;
    let payload = STANDARD
        .decode(envelope.manifest)
        .map_err(|_| "Invalid update payload")?;
    let key = STANDARD
        .decode(public_key)
        .map_err(|_| "Invalid update verification key")?;
    let sig = STANDARD
        .decode(envelope.signature)
        .map_err(|_| "Invalid update signature")?;
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(&payload, &sig)
        .map_err(|_| "The update signature could not be verified. Nothing was installed.")?;
    let m: Manifest =
        serde_json::from_slice(&payload).map_err(|_| "Invalid signed update metadata")?;
    if m.schema != 1
        || m.product != "AetherDesk"
        || m.target != "windows-x64"
        || m.published > now + 300
        || m.expires <= now
        || m.expires <= m.published
        || m.expires
            .checked_sub(m.published)
            .is_none_or(|age| age > 366 * 86400)
        || m.notes.len() > 32_768
        || m.files.is_empty()
        || m.files.len() > 30_000
    {
        return Err("This update is expired, incompatible or invalid".into());
    }
    version(&m.version, m.build)?;
    for asset in std::iter::once(&m.portable).chain(m.installer.iter()) {
        if !relative(&asset.name)
            || asset.name.contains('/')
            || !hash_valid(&asset.sha256)
            || asset.bytes == 0
            || asset.bytes > MAX_PACKAGE
        {
            return Err("Invalid update package declaration".into());
        }
    }
    if !m.portable.name.ends_with(".zip")
        || m.installer
            .as_ref()
            .is_some_and(|a| !a.name.ends_with(".exe"))
    {
        return Err("Invalid update package type".into());
    }
    let mut names = HashSet::new();
    let mut total = 0_u64;
    for file in &m.files {
        if !relative(&file.path)
            || !hash_valid(&file.sha256)
            || file.bytes > MAX_PACKAGE
            || !names.insert(file.path.to_ascii_lowercase())
        {
            return Err("Unsafe or duplicated update file path".into());
        }
        total = total.checked_add(file.bytes).ok_or("Package too large")?;
    }
    for file in &m.files {
        let mut parent = Path::new(&file.path).parent();
        while let Some(path) = parent {
            if names.contains(
                &path
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_ascii_lowercase(),
            ) {
                return Err("Update file conflicts with a parent folder".into());
            }
            parent = path.parent();
        }
    }
    if total > 12 * 1024 * 1024 * 1024 || !names.contains("aetherdesk.exe") {
        return Err("Incomplete or oversized application update".into());
    }
    Ok(m)
}
pub fn file_hash(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| "Cannot read update file")?;
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| "Cannot read update file")?;
        if n == 0 {
            break;
        }
        context.update(&buffer[..n]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::{
        rand::SystemRandom,
        signature::{Ed25519KeyPair, KeyPair},
    };
    fn signed() -> (serde_json::Value, Ed25519KeyPair) {
        let key = Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        (
            serde_json::json!({"schema":1,"product":"AetherDesk","target":"windows-x64","version":"0.2.0","build":1,
            "published":1000,"expires":2000,"notes":"test","portable":{"name":"app.zip","bytes":10,"sha256":"a".repeat(64)},"installer":null,
            "files":[{"path":"AetherDesk.exe","bytes":5,"sha256":"b".repeat(64)}]}),
            key,
        )
    }
    fn envelope(value: &serde_json::Value, key: &Ed25519KeyPair) -> Vec<u8> {
        let payload = serde_json::to_vec(value).unwrap();
        serde_json::to_vec(&serde_json::json!({"manifest":STANDARD.encode(&payload),"signature":STANDARD.encode(key.sign(&payload).as_ref())})).unwrap()
    }
    #[test]
    fn signed_updates_reject_tampering_expiry_wrong_target_and_path_escape() {
        let (mut value, key) = signed();
        let public = STANDARD.encode(key.public_key().as_ref());
        let bytes = envelope(&value, &key);
        assert!(verify(&bytes, &public, 1500).is_ok());
        assert!(verify(&bytes, &public, 2000).is_err());
        let (_, other) = signed();
        assert!(verify(&bytes, &STANDARD.encode(other.public_key().as_ref()), 1500).is_err());
        for path in [
            "../outside",
            "C:/evil",
            "a\\b",
            "aux.txt",
            "a./b",
            "a//b",
            "a:b",
            "/absolute",
        ] {
            value["files"][0]["path"] = path.into();
            assert!(
                verify(&envelope(&value, &key), &public, 1500).is_err(),
                "{path}"
            );
        }
        value["files"][0]["path"] = "AetherDesk.exe".into();
        value["target"] = "linux".into();
        assert!(verify(&envelope(&value, &key), &public, 1500).is_err());
        assert!(version("0.1.2", 2).unwrap() > version("0.1.2", 1).unwrap());
        assert!(version("0.1.2.AI-R", 1).is_err());
    }
}
