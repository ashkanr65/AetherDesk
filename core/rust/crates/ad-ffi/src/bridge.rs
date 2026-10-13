use ad_ssh::{SshClient, SshConfig, SshSession, X11ForwardingConfig};
use lazy_static::lazy_static;
use parking_lot::Mutex as ParkingMutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::Arc;
use tokio::runtime::{Builder, Runtime};
use tracing::warn;
use uuid::Uuid;

type SharedSession = Arc<SshSession>;

const SHELL_INTEGRATION_BOOTSTRAP: &str = r#"__aetherdesk_emit_cwd(){ local __ad_status=$?; printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"; return "$__ad_status"; }; case ";${PROMPT_COMMAND:-};" in *';__aetherdesk_emit_cwd;'*) ;; *) PROMPT_COMMAND="__aetherdesk_emit_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;; esac; __aetherdesk_emit_cwd"#;

lazy_static! {
    pub(crate) static ref RUNTIME: Runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name("aetherdesk-core")
        .build()
        .expect("failed to create AetherDesk runtime");
    static ref SSH_SESSIONS: ParkingMutex<HashMap<String, SharedSession>> =
        ParkingMutex::new(HashMap::new());
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

pub(crate) fn set_error(message: impl Into<String>) {
    LAST_ERROR.with(|error| *error.borrow_mut() = message.into());
}

pub(crate) fn clear_error() {
    LAST_ERROR.with(|error| error.borrow_mut().clear());
}

pub(crate) fn into_c_string(value: impl AsRef<str>) -> *mut c_char {
    let sanitized = value.as_ref().replace('\0', "�");
    CString::new(sanitized)
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

pub(crate) fn required_string(pointer: *const c_char, field: &str) -> Result<String, String> {
    if pointer.is_null() {
        return Err(format!("{field} is required"));
    }
    let value = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .trim()
        .to_string();
    if value.is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(value)
    }
}

fn optional_string(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

pub(crate) fn session_by_id(session_id: *const c_char) -> Result<(String, SharedSession), String> {
    let id = required_string(session_id, "session id")?;
    let session = SSH_SESSIONS
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("SSH session {id} was not found"))?;
    Ok((id, session))
}

fn connect_ssh(config: SshConfig, open_shell: bool) -> *mut c_char {
    let total_timeout = std::time::Duration::from_secs(
        config
            .timeout_secs
            .max(1)
            .saturating_mul(2)
            .saturating_add(15),
    );
    let connection = RUNTIME.block_on(async move {
        tokio::time::timeout(total_timeout, async move {
            let client = SshClient::standalone(config);
            let mut session = if open_shell {
                client.connect().await?
            } else {
                client.connect_with_bulk_transport().await?
            };
            if open_shell {
                session
                    .open_shell_with_bootstrap(SHELL_INTEGRATION_BOOTSTRAP)
                    .await?;
            }
            Ok::<SshSession, Box<dyn std::error::Error + Send + Sync>>(session)
        })
        .await
        .map_err(|_| {
            format!(
                "SSH authentication or session setup timed out after {} seconds",
                total_timeout.as_secs()
            )
        })?
    });

    match connection {
        Ok(session) => {
            let id = Uuid::new_v4().to_string();
            SSH_SESSIONS.lock().insert(id.clone(), Arc::new(session));
            into_c_string(id)
        }
        Err(error) => {
            set_error(error.to_string());
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_ssh_connect(
    host: *const c_char,
    port: u16,
    username: *const c_char,
    password: *const c_char,
) -> *mut c_char {
    clear_error();
    let host = match required_string(host, "host") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let username = match required_string(username, "username") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let password = optional_string(password);
    connect_ssh(
        SshConfig {
            host,
            port: if port == 0 { 22 } else { port },
            username,
            password: (!password.is_empty()).then_some(password),
            ..Default::default()
        },
        true,
    )
}

/// Extended SSH entry point used by the desktop application. Authentication
/// secrets are supplied only for this call and are never persisted by Rust.
#[no_mangle]
pub extern "C" fn ad_ssh_connect_ex(
    host: *const c_char,
    port: u16,
    username: *const c_char,
    secret: *const c_char,
    authentication: *const c_char,
    key_path: *const c_char,
    trust_new_host_key: c_int,
    open_shell: c_int,
) -> *mut c_char {
    clear_error();
    let host = match required_string(host, "host") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let username = match required_string(username, "username") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let secret = optional_string(secret);
    let authentication = optional_string(authentication);
    let key_path = optional_string(key_path);
    let mut config = SshConfig {
        host,
        port: if port == 0 { 22 } else { port },
        username,
        trust_new_host_key: trust_new_host_key != 0,
        ..Default::default()
    };

    match authentication.as_str() {
        "password" | "" => {
            config.password = (!secret.is_empty()).then_some(secret);
        }
        "sshKey" => {
            if key_path.trim().is_empty() {
                set_error("private key path is required");
                return std::ptr::null_mut();
            }
            config.key_path = Some(key_path);
            config.key_passphrase = (!secret.is_empty()).then_some(secret);
        }
        unsupported => {
            set_error(format!(
                "authentication method {unsupported} is not available in this build"
            ));
            return std::ptr::null_mut();
        }
    }

    connect_ssh(config, open_shell != 0)
}

/// Extended SSH entry point with a secured local X11 server target.
#[no_mangle]
pub extern "C" fn ad_ssh_connect_ex2(
    host: *const c_char,
    port: u16,
    username: *const c_char,
    secret: *const c_char,
    authentication: *const c_char,
    key_path: *const c_char,
    trust_new_host_key: c_int,
    open_shell: c_int,
    x11_local_port: u16,
    x11_authentication_protocol: *const c_char,
    x11_authentication_cookie: *const c_char,
    x11_screen_number: u32,
) -> *mut c_char {
    ad_ssh_connect_ex3(
        host,
        port,
        username,
        secret,
        authentication,
        key_path,
        trust_new_host_key,
        open_shell,
        x11_local_port,
        x11_authentication_protocol,
        x11_authentication_cookie,
        x11_screen_number,
        std::ptr::null(),
    )
}

#[no_mangle]
pub extern "C" fn ad_ssh_connect_ex3(
    host: *const c_char,
    port: u16,
    username: *const c_char,
    secret: *const c_char,
    authentication: *const c_char,
    key_path: *const c_char,
    trust_new_host_key: c_int,
    open_shell: c_int,
    x11_local_port: u16,
    x11_authentication_protocol: *const c_char,
    x11_authentication_cookie: *const c_char,
    x11_screen_number: u32,
    wireguard_config_path: *const c_char,
) -> *mut c_char {
    clear_error();
    let host = match required_string(host, "host") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let username = match required_string(username, "username") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let secret = optional_string(secret);
    let authentication = optional_string(authentication);
    let key_path = optional_string(key_path);
    let mut config = SshConfig {
        host,
        port: if port == 0 { 22 } else { port },
        username,
        trust_new_host_key: trust_new_host_key != 0,
        ..Default::default()
    };

    match authentication.as_str() {
        "password" | "" => config.password = (!secret.is_empty()).then_some(secret),
        "sshKey" => {
            if key_path.trim().is_empty() {
                set_error("private key path is required");
                return std::ptr::null_mut();
            }
            config.key_path = Some(key_path);
            config.key_passphrase = (!secret.is_empty()).then_some(secret);
        }
        unsupported => {
            set_error(format!(
                "authentication method {unsupported} is not available in this build"
            ));
            return std::ptr::null_mut();
        }
    }

    if x11_local_port != 0 {
        let protocol = optional_string(x11_authentication_protocol);
        let cookie = optional_string(x11_authentication_cookie);
        if protocol.is_empty() || cookie.is_empty() {
            set_error("X11 authentication protocol and cookie are required");
            return std::ptr::null_mut();
        }
        config.x11 = Some(X11ForwardingConfig {
            local_port: x11_local_port,
            authentication_protocol: protocol,
            authentication_cookie: Uuid::new_v4().simple().to_string(),
            local_authentication_cookie: cookie,
            screen_number: x11_screen_number,
        });
    }

    let vpn_path = optional_string(wireguard_config_path);
    config.wireguard_config_path = (!vpn_path.is_empty()).then_some(vpn_path);
    connect_ssh(config, open_shell != 0)
}

#[no_mangle]
pub extern "C" fn ad_ssh_start_local_forward(
    session_id: *const c_char,
    local_host: *const c_char,
    remote_host: *const c_char,
    remote_port: u16,
) -> u16 {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let remote_host = match required_string(remote_host, "remote host") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let local_host = match required_string(local_host, "local host") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    if remote_port == 0 {
        set_error("remote port is required");
        return 0;
    }
    match RUNTIME.block_on(session.start_local_forward(&local_host, &remote_host, remote_port)) {
        Ok(local_port) => local_port,
        Err(error) => {
            set_error(error.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_ssh_execute_command(
    session_id: *const c_char,
    command: *const c_char,
) -> *mut c_char {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let command = match required_string(command, "command") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    match RUNTIME.block_on(async { session.execute_command(&command).await }) {
        Ok(output) => into_c_string(output),
        Err(error) => {
            set_error(error.to_string());
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_ssh_write(session_id: *const c_char, data: *const c_char) -> c_int {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    if data.is_null() {
        set_error("terminal data is required");
        return -1;
    }
    let bytes = unsafe { CStr::from_ptr(data) }.to_bytes().to_vec();
    match session.send_data(&bytes) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error.to_string());
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_ssh_read(session_id: *const c_char) -> *mut c_char {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    match session.read_available(64 * 1024) {
        Ok(output) => into_c_string(String::from_utf8_lossy(&output)),
        Err(error) => {
            set_error(error.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Binary-safe terminal input. Copies the bytes before returning to the caller.
/// # Safety
/// `data` must point to `length` readable bytes and `session_id` to a C string.
#[no_mangle]
pub unsafe extern "C" fn ad_ssh_write_bytes(
    session_id: *const c_char,
    data: *const u8,
    length: usize,
) -> c_int {
    clear_error();
    let result = (|| -> Result<(), String> {
        if data.is_null() || length > 1024 * 1024 {
            return Err("Invalid terminal input buffer (maximum 1 MiB)".into());
        }
        let (_, session) = session_by_id(session_id)?;
        session
            .send_data(std::slice::from_raw_parts(data, length))
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

/// Drain buffered bytes without waiting for the network. Returns -1 on failure.
/// # Safety
/// `data` must point to `capacity` writable bytes and `session_id` to a C string.
#[no_mangle]
pub unsafe extern "C" fn ad_ssh_read_bytes(
    session_id: *const c_char,
    data: *mut u8,
    capacity: usize,
) -> c_int {
    clear_error();
    let result = (|| -> Result<usize, String> {
        if data.is_null() || capacity == 0 || capacity > 1024 * 1024 {
            return Err("Invalid terminal output buffer".into());
        }
        let (_, session) = session_by_id(session_id)?;
        let bytes = session
            .read_available(capacity)
            .map_err(|e| e.to_string())?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        Ok(bytes.len())
    })();
    match result {
        Ok(length) => length as c_int,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_ssh_resize(session_id: *const c_char, cols: u16, rows: u16) -> c_int {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    match session.resize_pty(cols, rows) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error.to_string());
            -1
        }
    }
}

pub(crate) fn session_is_registered(id: &str) -> bool {
    SSH_SESSIONS.lock().contains_key(id)
}

#[no_mangle]
pub extern "C" fn ad_ssh_disconnect(session_id: *const c_char) -> c_int {
    clear_error();
    let id = match required_string(session_id, "session id") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let Some(session) = SSH_SESSIONS.lock().remove(&id) else {
        set_error(format!("SSH session {id} was not found"));
        return -1;
    };
    crate::file_workspace::close_session(&id);
    match RUNTIME.block_on(async { session.disconnect().await }) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error.to_string());
            -1
        }
    }
}

/// Close every open SSH session synchronously. Called during application exit
/// so no shell, SFTP subsystem or forwarded channel outlives the client on the
/// remote host. Returns the number of sessions that could not be closed.
#[no_mangle]
pub extern "C" fn ad_ssh_shutdown_all() -> c_int {
    clear_error();
    let sessions: Vec<(String, SharedSession)> = SSH_SESSIONS.lock().drain().collect();
    RUNTIME.block_on(async move {
        let mut failures = 0;
        for (id, session) in sessions {
            crate::file_workspace::close_session(&id);
            match tokio::time::timeout(std::time::Duration::from_secs(3), session.disconnect())
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!("SSH session {id} reported an error while closing: {error}");
                    failures += 1;
                }
                Err(_) => {
                    warn!("SSH session {id} did not close within 3 seconds");
                    failures += 1;
                }
            }
        }
        failures
    })
}

#[no_mangle]
pub extern "C" fn ad_sftp_list(
    session_id: *const c_char,
    remote_path: *const c_char,
) -> *mut c_char {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return std::ptr::null_mut();
        }
    };
    let path = optional_string(remote_path);
    let path = if path.trim().is_empty() {
        "."
    } else {
        path.as_str()
    };
    match RUNTIME.block_on(async { session.sftp_list(path).await }) {
        Ok(entries) => match serde_json::to_string(&entries) {
            Ok(json) => into_c_string(json),
            Err(error) => {
                set_error(error.to_string());
                std::ptr::null_mut()
            }
        },
        Err(error) => {
            set_error(error.to_string());
            std::ptr::null_mut()
        }
    }
}

fn sftp_upload_file(
    session_id: *const c_char,
    local_path: *const c_char,
    remote_path: *const c_char,
    resume: bool,
) -> c_int {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let local_path = match required_string(local_path, "local path") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let remote_path = match required_string(remote_path, "remote path") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    match RUNTIME.block_on(async {
        session
            .sftp_upload_resumable(&local_path, &remote_path, resume)
            .await
    }) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error.to_string());
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_sftp_upload_file(
    session_id: *const c_char,
    local_path: *const c_char,
    remote_path: *const c_char,
) -> c_int {
    sftp_upload_file(session_id, local_path, remote_path, false)
}

#[no_mangle]
pub extern "C" fn ad_sftp_upload_file_resume(
    session_id: *const c_char,
    local_path: *const c_char,
    remote_path: *const c_char,
) -> c_int {
    sftp_upload_file(session_id, local_path, remote_path, true)
}

fn sftp_download_file(
    session_id: *const c_char,
    remote_path: *const c_char,
    local_path: *const c_char,
    resume: bool,
) -> c_int {
    clear_error();
    let (_, session) = match session_by_id(session_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let remote_path = match required_string(remote_path, "remote path") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let local_path = match required_string(local_path, "local path") {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    match RUNTIME.block_on(async {
        session
            .sftp_download_resumable(&remote_path, &local_path, resume)
            .await
    }) {
        Ok(()) => 0,
        Err(error) => {
            set_error(error.to_string());
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_sftp_download_file(
    session_id: *const c_char,
    remote_path: *const c_char,
    local_path: *const c_char,
) -> c_int {
    sftp_download_file(session_id, remote_path, local_path, false)
}

#[no_mangle]
pub extern "C" fn ad_sftp_download_file_resume(
    session_id: *const c_char,
    remote_path: *const c_char,
    local_path: *const c_char,
) -> c_int {
    sftp_download_file(session_id, remote_path, local_path, true)
}

#[no_mangle]
pub extern "C" fn ad_last_error() -> *mut c_char {
    LAST_ERROR.with(|error| into_c_string(error.borrow().as_str()))
}

#[no_mangle]
pub extern "C" fn ad_get_version() -> *mut c_char {
    into_c_string(env!("CARGO_PKG_VERSION"))
}

#[no_mangle]
pub extern "C" fn ad_get_capabilities() -> *mut c_char {
    into_c_string(format!(
        r#"{{"ssh":true,"sftp":true,"password":true,"sshKey":true,"ed25519":true,"remoteDesktop":{},"sshX11":true,"bundledX11":false,"smartCard":false,"mfa":false,"fido2":false}}"#,
        crate::remote::available()
    ))
}

#[no_mangle]
pub extern "C" fn ad_free_string(pointer: *mut c_char) {
    if !pointer.is_null() {
        unsafe {
            drop(CString::from_raw(pointer));
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_rdp_connect(
    _host: *const c_char,
    _port: u16,
    _username: *const c_char,
    _password: *const c_char,
) -> *mut c_char {
    set_error("RDP engine is not implemented in this release");
    std::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn ad_vnc_connect(
    _host: *const c_char,
    _port: u16,
    _password: *const c_char,
) -> *mut c_char {
    set_error("VNC engine is not implemented in this release");
    std::ptr::null_mut()
}
