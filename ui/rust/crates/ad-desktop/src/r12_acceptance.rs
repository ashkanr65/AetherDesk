//! Explicit, ignored live acceptance checks. Ordinary builds never contact a host.
//! The runner supplies a trusted host fingerprint and references the user's key
//! in place. Files and desktop UUIDs are disposable and independent of user work.
use ad_ssh::{FileProgress, RemoteFileRequest, SshClient, SshConfig, SshSession};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn config() -> Value {
    let path = std::env::var_os("AETHERDESK_R12_CONFIG").expect("Explicit R12 config required");
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    std::fs::create_dir_all(value["reportDirectory"].as_str().unwrap()).unwrap();
    value
}
fn ssh_config(value: &Value) -> SshConfig {
    SshConfig {
        host: value["host"].as_str().unwrap().into(),
        username: value["username"].as_str().unwrap().into(),
        key_path: Some(value["identityFile"].as_str().unwrap().into()),
        confirmed_host_fingerprint: Some(value["hostFingerprint"].as_str().unwrap().into()),
        timeout_secs: 15,
        ..Default::default()
    }
}
fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn report(name: &str, value: &Value, result: &Result<Value, String>) {
    let directory = PathBuf::from(value["reportDirectory"].as_str().unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    let output = match result {
        Ok(details) => json!({"check":name,"passed":true,"details":details}),
        Err(error) => json!({"check":name,"passed":false,"error":error}),
    };
    std::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&output).unwrap(),
    )
    .unwrap();
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
async fn command(session: &SshSession, text: &str) -> Result<String, String> {
    let result = session
        .execute_command_output(text, Duration::from_secs(15))
        .await
        .map_err(|e| e.to_string())?;
    check(result.exit_status == 0, "Live SSH command failed")?;
    Ok(result.stdout_text())
}
async fn wait_terminal(session: &SshSession, marker: &str) -> Result<String, String> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut output = Vec::new();
        loop {
            output.extend(
                session
                    .read_available(64 * 1024)
                    .map_err(|e| e.to_string())?,
            );
            if String::from_utf8_lossy(&output).contains(marker) {
                return Ok(String::from_utf8_lossy(&output).into_owned());
            }
            if output.len() > 1024 * 1024 {
                return Err("Unexpectedly large terminal output".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "An isolated terminal update waited for another input".to_owned())?
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Requires explicitly configured live SSH host and existing private key"]
async fn live_ssh_sftp_editor_tools_and_cleanup() {
    let value = config();
    let result = ssh_and_files(&value).await;
    report("live-ssh-sftp", &value, &result);
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}
async fn ssh_and_files(value: &Value) -> Result<Value, String> {
    let mut session = SshClient::standalone(ssh_config(value))
        .connect()
        .await
        .map_err(|e| e.to_string())?;
    let other = SshClient::standalone(ssh_config(value))
        .connect()
        .await
        .map_err(|e| e.to_string())?;
    let directory = command(&session, "mktemp -d /tmp/aetherdesk-r12.XXXXXXXX")
        .await?
        .trim()
        .to_owned();
    check(
        directory.starts_with("/tmp/aetherdesk-r12.")
            && directory
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/.-".contains(&b)),
        "Unexpected disposable host directory",
    )?;
    let local = tempfile::tempdir().map_err(|e| e.to_string())?;
    let result=async {
        let identity=command(&session,"hostname; whoami").await?;
        check(identity.lines().any(|v|v=="fic") && identity.lines().any(|v|v=="AR"),"Unexpected live SSH identity")?;
        let mut mismatch=ssh_config(value);
        mismatch.confirmed_host_fingerprint=Some("SHA256:R12IncorrectFingerprint".into());
        check(SshClient::standalone(mismatch).connect().await.is_err(),"Changed host fingerprint was accepted")?;
        session.open_shell().await.map_err(|e|e.to_string())?;
        let mut latencies=Vec::new();
        for index in 0..5 {
            let marker=format!("R12_INPUT_{index}_DONE");
            let started=std::time::Instant::now();
            // Octal escaping prevents the PTY echo from satisfying the probe.
            let text=format!("printf '\\12212_INPUT_{index}_DONE\\n'\r");
            session.send_data(text.as_bytes()).map_err(|e|e.to_string())?;
            wait_terminal(&session,&marker).await?;
            latencies.push(started.elapsed().as_millis());
        }
        session.resize_pty(113,37).map_err(|e|e.to_string())?;
        session.send_data(b"stty size; printf '\\12212_GEOMETRY_DONE\\n'\r").map_err(|e|e.to_string())?;
        let geometry=wait_terminal(&session,"R12_GEOMETRY_DONE").await?;
        check(geometry.contains("37 113"),"Terminal resize did not reach the host PTY")?;
        let source=local.path().join("payload.bin");
        let bytes=(0..1_048_576).map(|n|(n%251)as u8).collect::<Vec<_>>();
        std::fs::write(&source,&bytes).map_err(|e|e.to_string())?;
        let remote=format!("{directory}/payload.bin");
        session.sftp_upload(source.to_str().unwrap(),&remote).await.map_err(|e|e.to_string())?;
        let download=local.path().join("received.bin");
        session.sftp_download(&remote,download.to_str().unwrap()).await.map_err(|e|e.to_string())?;
        check(std::fs::read(&download).map_err(|e|e.to_string())?==bytes,"Live SFTP changed binary data")?;
        let text=local.path().join("edit.txt");
        std::fs::write(&text,"R12 original\n").map_err(|e|e.to_string())?;
        let remote_text=format!("{directory}/edit.txt");
        session.sftp_upload(text.to_str().unwrap(),&remote_text).await.map_err(|e|e.to_string())?;
        let mut opened=request("download",&remote_text);
        opened.local_path=Some(local.path().join("opened.txt").to_string_lossy().into_owned());
        let metadata=session.file_operation(opened,Arc::new(FileProgress::default())).await.map_err(|e|e.to_string())?.metadata.ok_or("Missing remote revision")?;
        std::fs::write(&text,"R12 saved directly to host — λ\n").map_err(|e|e.to_string())?;
        let mut save=request("save",&remote_text);
        save.local_path=Some(text.to_string_lossy().into_owned());
        save.expected_revision=Some(metadata.revision.ok_or("Opened file had no full content revision")?);
        let saved=session.file_operation(save.clone(),Arc::new(FileProgress::default())).await.map_err(|e|e.to_string())?;
        check(saved.metadata.is_some(),"Remote editor save was not acknowledged")?;
        check(session.file_operation(save,Arc::new(FileProgress::default())).await.is_err(),"A stale remote editor revision overwrote the host file")?;
        session.sftp_download(&remote_text,download.to_str().unwrap()).await.map_err(|e|e.to_string())?;
        check(std::fs::read(&download).map_err(|e|e.to_string())?==std::fs::read(&text).map_err(|e|e.to_string())?,"Editor save did not reach host")?;
        let tools=crate::ai_tools::Workspace::Ssh {session:Arc::new(other),root:directory.clone(),label:"R12".into(),id:"r12".into()};
        let output=crate::ai_tools::execute(tools.clone(),"run_command".into(),json!({"command":"printf R12_TOOL_COMMAND"})).await?;
        check(output.to_string().contains("R12_TOOL_COMMAND"),"Native SSH assistant command failed")?;
        session.close().await.map_err(|e|e.to_string())?;
        if let crate::ai_tools::Workspace::Ssh{session:other,..}=tools {
            check(command(&other,"printf R12_DUPLICATE_ALIVE").await?=="R12_DUPLICATE_ALIVE","Closing one SSH session stopped another")?;
            other.close().await.map_err(|e|e.to_string())?;
        }
        Ok(json!({"verifiedFingerprint":true,"wrongFingerprintRejected":true,"isolatedInputLatencyMs":latencies,
            "binaryBytesRoundTripped":bytes.len(),"editorAtomicSaveAndConflict":true,"assistantSshCommand":true,"duplicateConnectionIsolation":true}))
    }.await;
    // Reconnect for cleanup even if a case closed its channel or failed.
    if let Ok(cleanup) = SshClient::standalone(ssh_config(value)).connect().await {
        let cleanup_result=command(&cleanup,&format!("test ! -L '{directory}' && test \"$(stat -c %u '{directory}')\" = \"$(id -u)\" && rm -rf -- '{directory}'")).await;
        let _ = cleanup.close().await;
        cleanup_result?;
    } else {
        return Err("Could not reconnect to clean the disposable host directory".into());
    }
    let _ = session.close().await;
    result
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Requires live SSH forwarding and the freshly bundled X11 runtime"]
async fn live_x11_forwarding_and_cleanup() {
    let value = config();
    let result=async {
        let runtime=Arc::new(crate::x11::X11Runtime::default());
        let mut lease=runtime.acquire(&CancellationToken::new()).await?;
        let mut parameters=ssh_config(&value);
        parameters.x11=Some(lease.config.clone());
        let mut session=SshClient::standalone(parameters).connect().await.map_err(|e|e.to_string())?;
        let probe=async {
            session.open_shell_with_bootstrap("xclock -name AetherDeskR12 >/dev/null 2>&1 & r12_clock=$!; sleep 2; if kill -0 $r12_clock 2>/dev/null; then kill $r12_clock; wait $r12_clock 2>/dev/null; printf R12_X11_AUTHENTICATED; else printf R12_X11_FAILED; fi").await.map_err(|e|e.to_string())?;
            wait_terminal(&session,"R12_X11_AUTHENTICATED").await?;
            check(lease.is_running()?,"The bundled X11 server stopped during live forwarding")?;
            Ok(json!({"bundledAuthenticatedDisplay":true,"liveHostXclock":true,"separateBulkTransport":true}))
        }.await;
        let closed=session.close().await.map_err(|e|e.to_string());
        lease.stop().await;
        closed?;
        probe
    }.await;
    report("live-x11", &value, &result);
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}

async fn desktop_frame(
    client: &crate::remote::RemoteClient,
    size: (u32, u32),
) -> Result<crate::remote::Image, String> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(error) = client.peek_status().error {
                return Err(error);
            }
            if let Some(frame) = client.frame() {
                if (frame.width, frame.height) == size {
                    check(
                        frame.pixels.len() == size.0 as usize * size.1 as usize * 4,
                        "Malformed live desktop frame",
                    )?;
                    return Ok(frame);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "Live desktop did not produce the expected geometry".to_owned())?
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Requires the bundled Linux agent and an isolated live desktop UUID"]
async fn live_desktop_resize_detach_reconnect_and_end() {
    let value = config();
    let result=async {
        let session=Arc::new(SshClient::standalone(ssh_config(&value)).connect().await.map_err(|e|e.to_string())?);
        let id=uuid::Uuid::parse_str(value["sessionId"].as_str().unwrap()).map_err(|e|e.to_string())?;
        let cancel=CancellationToken::new();
        let started=std::time::Instant::now();
        let client=crate::remote::RemoteClient::start(session.clone(),"xfce".into(),id,eframe::egui::Context::default(),&cancel).await?;
        let measured=async {
            let first=desktop_frame(&client,(1280,720)).await?;
            let first_frame_ms=started.elapsed().as_millis();
            let mut geometry=Vec::new();
            for (width,height,monitors) in [(1366,768,1),(2560,720,2),(1280,720,1)] {
                client.send(crate::remote::Request::MonitorLayout {properties:vec![],width,height,monitors:(0..monitors).map(|index|ad_stream_protocol::Rectangle{x:index*width/monitors,y:0,width:width/monitors,height}).collect()})?;
                let frame=desktop_frame(&client,(width,height)).await?;
                geometry.push(json!({"width":frame.width,"height":frame.height,"sequence":frame.sequence}));
            }
            // Persist only our isolated empty test desktop, not the user's screen.
            let mut rgba=first.pixels.as_ref().clone();
            for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0,2); }
            let image=image::RgbaImage::from_raw(first.width,first.height,rgba).ok_or("Invalid screen capture")?;
            image.save(PathBuf::from(value["reportDirectory"].as_str().unwrap()).join("test-desktop.png")).map_err(|e|e.to_string())?;
            Ok::<_,String>(json!({"firstFrameMs":first_frame_ms,"decodedFrameBytes":first.pixels.len(),"monitorLayouts":geometry}))
        }.await;
        if measured.is_err() { let _=client.stop(true).await; let _=session.close().await; return measured; }
        client.stop(false).await?;
        let reconnect=crate::remote::RemoteClient::start(session.clone(),"xfce".into(),id,eframe::egui::Context::default(),&cancel).await;
        let client=reconnect?;
        check(!client.newly_created(),"Detach/reconnect replaced the retained desktop")?;
        let frame_result=desktop_frame(&client,(1280,720)).await;
        let end=client.stop(true).await;
        let closed=session.close().await.map_err(|e|e.to_string());
        frame_result?; end?; closed?;
        let cleanup=SshClient::standalone(ssh_config(&value)).connect().await.map_err(|e|e.to_string())?;
        let gone=command(&cleanup,&format!("test ! -e \"$HOME/.aetherdesk/sessions/{id}\" && printf R12_ENDED")).await;
        let _=cleanup.close().await;
        check(gone?=="R12_ENDED","Ended live desktop retained its session directory")?;
        let mut details=measured?;
        details["detachReconnectEndAcknowledgement"]=json!(true);
        Ok(details)
    }.await;
    report("live-desktop", &value, &result);
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "Uses a saved provider key for explicit tiny live API requests"]
async fn live_saved_provider_stream_and_file_tool() {
    let value = config();
    let result=async {
        let id=value["providerId"].as_str().ok_or("Provider not selected")?;
        let key=match crate::vault::read(id)? {
            Some(key)=>zeroize::Zeroizing::new(key),
            None=>crate::migration::read_secret(&format!("AetherDesk/assistant/provider/{id}"),"")?.ok_or("No saved API key is available")?.1,
        };
        let base=value["base"].as_str().unwrap();
        let model=value["model"].as_str().unwrap();
        let models=if value["providerCatalogPreviouslyVerified"].as_bool()==Some(true) { Vec::new() } else {
            let models=crate::ai_api::models(base,key.clone()).await?;
            check(models.iter().any(|entry|entry.id==model),"The selected live model is no longer in the provider catalog")?;
            models
        };
        let local=tempfile::tempdir().map_err(|e|e.to_string())?;
        std::fs::write(local.path().join("r12-proof.txt"),"R12_FILE_TOOL_OK").map_err(|e|e.to_string())?;
        let schema=crate::ai_tools::schemas().as_array().unwrap().iter().find(|tool|tool["function"]["name"]=="read_file").unwrap().clone();
        let message=json!({"role":"user","content":"Read r12-proof.txt using read_file. Do not guess its content."});
        let (events,_receive)=tokio::sync::mpsc::channel(256);
        let response=crate::ai_api::complete(base,key.clone(),json!({"model":model,"stream":true,"max_tokens":512,"reasoning":{"enabled":false},"messages":[message.clone()],"tools":[schema],"tool_choice":{"type":"function","function":{"name":"read_file"}}}),events).await?;
        let call=response["tool_calls"].as_array().and_then(|calls|calls.first()).ok_or("Live model returned no requested file tool call")?;
        check(call["function"]["name"]=="read_file","Live model requested an unexpected tool")?;
        let args:Value=serde_json::from_str(call["function"]["arguments"].as_str().ok_or("Tool arguments missing")?).map_err(|e|e.to_string())?;
        check(args["path"]=="r12-proof.txt","Live model changed the test file path")?;
        let tool=crate::ai_tools::execute(crate::ai_tools::Workspace::Local(local.path().into()),"read_file".into(),args).await?;
        check(tool.to_string().contains("R12_FILE_TOOL_OK"),"Native assistant tool failed to read the private fixture")?;
        let (events,_receive)=tokio::sync::mpsc::channel(256);
        let answer=crate::ai_api::complete(base,key,json!({"model":model,"stream":true,"max_tokens":512,"reasoning":{"enabled":false},"messages":[message,{"role":"assistant","content":response["content"],"tool_calls":response["tool_calls"]},{"role":"tool","tool_call_id":call["id"],"content":tool.to_string()},{"role":"user","content":"Reply with the exact file content, without explanation."}]}),events).await?;
        check(answer["content"].as_str().is_some_and(|text|text.contains("R12_FILE_TOOL_OK")),&format!("Live model did not use the tool result ({} reply characters)",answer["content"].as_str().unwrap_or_default().len()))?;
        Ok(json!({"provider":id,"model":model,"catalogModels":models.len(),"requestCount":if value["providerCatalogPreviouslyVerified"].as_bool()==Some(true){2}else{3},"streamedReply":true,"nativeFileToolRoundTrip":true,"secretPrintedOrModified":false}))
    }.await;
    report("live-provider", &value, &result);
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}
