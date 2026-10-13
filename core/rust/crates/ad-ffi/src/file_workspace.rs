use crate::bridge::{
    clear_error, into_c_string, required_string, session_by_id, set_error, RUNTIME,
};
use ad_ssh::{FileProgress, LanguageServer, LanguageServerConfig, RemoteFileRequest};
use lazy_static::lazy_static;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    os::raw::c_char,
    sync::{atomic::Ordering, Arc},
};

struct Task {
    owner: String,
    progress: Arc<FileProgress>,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
}
lazy_static! {
    static ref TASKS: Mutex<HashMap<String, Task>> = Mutex::new(HashMap::new());
}

#[no_mangle]
pub extern "C" fn ad_file_start(session_id: *const c_char, request: *const c_char) -> *mut c_char {
    clear_error();
    let result = (|| {
        let (owner, session) = session_by_id(session_id)?;
        let request: RemoteFileRequest =
            serde_json::from_str(&required_string(request, "request")?)
                .map_err(|e| e.to_string())?;
        let progress = Arc::new(FileProgress::default());
        let result = Arc::new(Mutex::new(None));
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut tasks = TASKS.lock();
            if !crate::bridge::session_is_registered(&owner) {
                return Err("SSH session disconnected".to_string());
            }
            if tasks.len() >= 64 {
                return Err(
                    "Too many file operations; finish or cancel an operation first".to_string(),
                );
            }
            tasks.insert(
                id.clone(),
                Task {
                    owner,
                    progress: progress.clone(),
                    result: result.clone(),
                },
            );
        }
        RUNTIME.spawn(async move {
            let outcome = session.file_operation(request, progress).await;
            *result.lock() = Some(match outcome {
                Ok(value) => serde_json::to_value(value).map_err(|e| e.to_string()),
                Err(error) => Err(error.to_string()),
            });
        });
        Ok(json!({"id":id}).to_string())
    })();
    match result {
        Ok(json) => into_c_string(json),
        Err(error) => {
            set_error(error);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_file_poll(id: *const c_char) -> *mut c_char {
    clear_error();
    let id = match required_string(id, "task id") {
        Ok(id) => id,
        Err(e) => {
            set_error(e);
            return std::ptr::null_mut();
        }
    };
    let mut tasks = TASKS.lock();
    let Some(task) = tasks.get(&id) else {
        set_error("File operation not found");
        return std::ptr::null_mut();
    };
    let result = task.result.lock().clone();
    let response = match &result {
        Some(Ok(value)) => json!({"state":"completed","result":value}),
        Some(Err(error)) => json!({"state":"failed","error":error}),
        None => {
            json!({"state":"running","transferred":task.progress.transferred.load(Ordering::Relaxed),"total":task.progress.total.load(Ordering::Relaxed)})
        }
    };
    if result.is_some() {
        tasks.remove(&id);
    }
    into_c_string(response.to_string())
}

#[no_mangle]
pub extern "C" fn ad_file_cancel(id: *const c_char) -> i32 {
    clear_error();
    let id = match required_string(id, "task id") {
        Ok(id) => id,
        Err(e) => {
            set_error(e);
            return -1;
        }
    };
    if let Some(task) = TASKS.lock().get(&id) {
        task.progress.cancel();
        0
    } else {
        // Cancellation is idempotent when completion polling won the race.
        0
    }
}

lazy_static! {
    static ref LANGUAGE_SERVERS: Mutex<HashMap<String, (String, LanguageServer)>> =
        Mutex::new(HashMap::new());
}

#[no_mangle]
pub extern "C" fn ad_lsp_start(session_id: *const c_char, config: *const c_char) -> *mut c_char {
    clear_error();
    let result = (|| {
        let (owner, session) = session_by_id(session_id)?;
        let config: LanguageServerConfig =
            serde_json::from_str(&required_string(config, "config")?).map_err(|e| e.to_string())?;
        let server = RUNTIME
            .block_on(session.start_language_server(config))
            .map_err(|e| e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut servers = LANGUAGE_SERVERS.lock();
        if !crate::bridge::session_is_registered(&owner) {
            return Err("SSH session disconnected".to_string());
        }
        if servers.len() >= 16 {
            return Err("Too many active language servers".to_string());
        }
        servers.insert(id.clone(), (owner, server));
        Ok(json!({"id":id}).to_string())
    })();
    match result {
        Ok(value) => into_c_string(value),
        Err(error) => {
            set_error(error);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_lsp_send(id: *const c_char, message: *const c_char) -> i32 {
    clear_error();
    let result = (|| {
        let id = required_string(id, "language server id")?;
        let message: Value = serde_json::from_str(&required_string(message, "message")?)
            .map_err(|e| e.to_string())?;
        let servers = LANGUAGE_SERVERS.lock();
        let (_, server) = servers
            .get(&id)
            .ok_or_else(|| "Language server not found".to_string())?;
        server.send(message).map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_lsp_poll(id: *const c_char) -> *mut c_char {
    clear_error();
    let result = (|| {
        let id = required_string(id, "language server id")?;
        let servers = LANGUAGE_SERVERS.lock();
        let (_, server) = servers
            .get(&id)
            .ok_or_else(|| "Language server not found".to_string())?;
        Ok::<String, String>(server.poll().to_string())
    })();
    match result {
        Ok(value) => into_c_string(value),
        Err(error) => {
            set_error(error);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn ad_lsp_stop(id: *const c_char) -> i32 {
    clear_error();
    let result = (|| {
        let id = required_string(id, "language server id")?;
        if let Some((_, server)) = LANGUAGE_SERVERS.lock().remove(&id) {
            server.stop();
        }
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

// Remove registrations as well as cancelling workers: abandoned UI pollers
// must not consume the global operation limits after reconnecting.
pub(crate) fn close_session(owner: &str) {
    TASKS.lock().retain(|_, task| {
        if task.owner == owner {
            task.progress.cancel();
            false
        } else {
            true
        }
    });
    LANGUAGE_SERVERS.lock().retain(|_, (session, server)| {
        if session == owner {
            server.stop();
            false
        } else {
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disconnect_removes_only_its_own_abandoned_file_jobs() {
        let owner = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let ids: Vec<_> = (0..3).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        {
            let mut tasks = TASKS.lock();
            for (index, id) in ids.iter().enumerate() {
                tasks.insert(
                    id.clone(),
                    Task {
                        owner: if index == 2 {
                            other.clone()
                        } else {
                            owner.clone()
                        },
                        progress: Arc::new(FileProgress::default()),
                        result: Arc::new(Mutex::new(None)),
                    },
                );
            }
        }
        close_session(&owner);
        {
            let tasks = TASKS.lock();
            assert!(!tasks.contains_key(&ids[0]) && !tasks.contains_key(&ids[1]));
            assert!(tasks.contains_key(&ids[2]));
        }
        close_session(&owner); // repeated disconnect is harmless
        close_session(&other);
    }
}
