//! Native credentials use a separate namespace and Windows DPAPI. A master
//! password is never retained in the profile or Credential Manager.
use ad_desktop_model::profiles::SavedSession;
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{rngs::OsRng, RngCore};
use serde_json::json;
use zeroize::Zeroizing;

fn identity(profile: &SavedSession, username: &str) -> String {
    json!([
        profile.id,
        profile.host,
        profile.port,
        username,
        profile.authentication,
        profile.key_path
    ])
    .to_string()
}
fn entropy(
    identity: &str,
    master: &str,
    salt: &[u8],
    protected: bool,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut bytes = Zeroizing::new(identity.as_bytes().to_vec());
    if protected {
        if master.is_empty() {
            return Err("Enter the master password".into());
        }
        let mut derived = Zeroizing::new([0u8; 32]);
        ring::pbkdf2::derive(
            ring::pbkdf2::PBKDF2_HMAC_SHA256,
            std::num::NonZeroU32::new(210_000).unwrap(),
            salt,
            master.as_bytes(),
            derived.as_mut(),
        );
        bytes.extend_from_slice(derived.as_ref());
    }
    Ok(Zeroizing::new(
        ring::digest::digest(&ring::digest::SHA256, &bytes)
            .as_ref()
            .to_vec(),
    ))
}
pub fn remember(
    profile: &SavedSession,
    username: &str,
    secret: &str,
    master: &str,
) -> Result<(), String> {
    if !profile.remember_password {
        return Ok(());
    }
    if secret.len() > 2048 {
        return Err("Secret is too large to remember".into());
    }
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let id = identity(profile, username);
    let entropy = entropy(&id, master, &salt, profile.master_password_protected)?;
    let encrypted = protect(secret.as_bytes(), &entropy, true)?;
    let value = json!({"version":1,"masterPasswordProtected":profile.master_password_protected,"salt":STANDARD.encode(salt),"protectedData":STANDARD.encode(&encrypted)});
    crate::vault::write(&format!("ssh-{}", profile.id), &value.to_string())
}
pub fn recall(
    profile: &SavedSession,
    username: &str,
    master: &str,
) -> Result<Option<String>, String> {
    let Some(encoded) = crate::vault::read(&format!("ssh-{}", profile.id))? else {
        return Ok(None);
    };
    let encoded = Zeroizing::new(encoded);
    let v: serde_json::Value =
        serde_json::from_str(&encoded).map_err(|_| "Saved credential is damaged")?;
    if v["version"] != 1 {
        return Err("Unsupported saved credential version".into());
    }
    let protected = v["masterPasswordProtected"]
        .as_bool()
        .ok_or("Invalid credential protection")?;
    let salt = STANDARD
        .decode(v["salt"].as_str().ok_or("Invalid credential salt")?)
        .map_err(|_| "Invalid credential salt")?;
    if salt.len() != 16 {
        return Err("Invalid credential salt".into());
    }
    let encrypted = STANDARD
        .decode(
            v["protectedData"]
                .as_str()
                .ok_or("Invalid protected credential")?,
        )
        .map_err(|_| "Invalid protected credential")?;
    let entropy = entropy(&identity(profile, username), master, &salt, protected)?;
    let bytes = protect(&encrypted, &entropy, false)?;
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|_| "Invalid decrypted secret".into())
}
#[cfg(windows)]
fn protect(bytes: &[u8], entropy: &[u8], encrypt: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Blob {
        size: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
    let input = Blob {
        size: bytes.len().try_into().map_err(|_| "Credential too large")?,
        data: bytes.as_ptr() as *mut u8,
    };
    let entropy = Blob {
        size: entropy.len() as u32,
        data: entropy.as_ptr() as *mut u8,
    };
    let entropy_pointer = if entropy.size == 0 {
        std::ptr::null()
    } else {
        &entropy as *const Blob
    };
    let mut output = Blob {
        size: 0,
        data: std::ptr::null_mut(),
    };
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                entropy_pointer,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                entropy_pointer,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut output,
            )
        }
    };
    if result == 0 {
        return Err(if encrypt {"Windows could not protect the credential"}else{"Could not unlock this credential. Check the master password, username and connection settings."}.into());
    }
    if output.data.is_null() {
        return Err("Windows returned an invalid protected credential".into());
    }
    let data = unsafe { std::slice::from_raw_parts_mut(output.data, output.size as usize) };
    let owned = Zeroizing::new(data.to_vec());
    zeroize::Zeroize::zeroize(data);
    unsafe {
        LocalFree(output.data.cast());
    }
    Ok(owned)
}
pub(crate) fn unprotect_legacy(bytes: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    protect(bytes, entropy, false)
}
#[cfg(not(windows))]
fn protect(_: &[u8], _: &[u8], _: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    Err("Credential protection requires Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn master_protection_requires_a_password_and_is_bound_to_connection() {
        let salt = [7u8; 16];
        assert!(entropy("host-one", "", &salt, true).is_err());
        assert_ne!(
            *entropy("host-one", "master", &salt, true).unwrap(),
            *entropy("host-two", "master", &salt, true).unwrap()
        );
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_the_wrong_entropy() {
        let encrypted = protect(b"test secret", b"correct context", true).unwrap();
        assert_eq!(
            &*protect(&encrypted, b"correct context", false).unwrap(),
            b"test secret"
        );
        assert!(protect(&encrypted, b"different context", false).is_err());
    }
}
