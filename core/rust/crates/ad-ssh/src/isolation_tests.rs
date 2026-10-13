use super::*;
use russh::server::{self, Server as _};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Peer {
    id: usize,
    next_id: Arc<AtomicUsize>,
    sent: Arc<AtomicUsize>,
    roles: Arc<parking_lot::Mutex<Vec<(usize, &'static str)>>>,
    shells: HashSet<russh::ChannelId>,
}

impl server::Server for Peer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        let mut client = self.clone();
        client.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        client
    }
}

impl server::Handler for Peer {
    type Error = russh::Error;
    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        assert_eq!((user, password), ("test", "test"));
        Ok(server::Auth::Accept)
    }
    async fn channel_open_session(
        &mut self,
        _: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }
    async fn x11_request(
        &mut self,
        channel: russh::ChannelId,
        _: bool,
        _: &str,
        _: &str,
        _: u32,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.roles.lock().push((self.id, "x11"));
        session.channel_success(channel)?;
        Ok(())
    }
    async fn pty_request(
        &mut self,
        channel: russh::ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.roles.lock().push((self.id, "terminal"));
        session.channel_success(channel)?;
        Ok(())
    }
    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.shells.insert(channel);
        session.channel_success(channel)?;
        Ok(())
    }
    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.shells.contains(&channel) {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }
    async fn channel_close(
        &mut self,
        channel: russh::ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.shells.remove(&channel);
        self.roles.lock().push((self.id, "closed"));
        Ok(())
    }
    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        if command.starts_with(b"printf '__AETHERDESK_X11__") {
            session.data(
                channel,
                b"__AETHERDESK_X11__localhost:10.0\n/home/test/.Xauthority\n".to_vec(),
            )?;
        } else if command == b"never-exit" {
            // Hold the channel open until client cancellation sends CLOSE.
        } else if command == b"exec 'aetherdesk-test-lsp'" {
            self.shells.insert(channel);
        } else {
            session.data(channel, b"ready\n".to_vec())?;
            session.exit_status_request(channel, 0)?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<server::Msg>,
        _: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.roles
            .lock()
            .push((self.id, if port == 1 { "media" } else { "input" }));
        let sent = self.sent.clone();
        tokio::spawn(async move {
            if port == 1 {
                for _ in 0..1024 {
                    if channel.data_bytes(vec![42; 32768]).await.is_err() {
                        break;
                    }
                    sent.fetch_add(32768, Ordering::Relaxed);
                }
            } else {
                let mut stream = channel.into_stream();
                let mut buffer = [0; 128];
                while let Ok(count) = stream.read(&mut buffer).await {
                    if count == 0 || stream.write_all(&buffer[..count]).await.is_err() {
                        break;
                    }
                }
            }
        });
        Ok(())
    }
}

#[tokio::test]
async fn blocked_graphics_cannot_block_terminal_or_remote_input() {
    let key = russh::keys::PrivateKey::new(
        russh::keys::ssh_key::private::KeypairData::Ed25519(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
        ),
        "test",
    )
    .unwrap();
    let public_key = key.public_key().clone();
    let config = Arc::new(server::Config {
        keys: vec![key],
        nodelay: true,
        ..Default::default()
    });
    let sent = Arc::new(AtomicUsize::new(0));
    let roles = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut peer = Peer {
        id: 0,
        next_id: Arc::new(AtomicUsize::new(0)),
        sent: sent.clone(),
        roles: roles.clone(),
        shells: HashSet::new(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let running = peer.run_on_socket(config, &listener);
        let handle = running.handle();
        tokio::select! {
            result = running => result.unwrap(),
            _ = stop_rx => handle.shutdown("test complete".into()),
        }
    });
    let client = SshClient {
        config: SshConfig {
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            password: Some("test".into()),
            x11: Some(X11ForwardingConfig {
                local_port: 6010,
                authentication_protocol: "MIT-MAGIC-COOKIE-1".into(),
                authentication_cookie: "01".repeat(16),
                local_authentication_cookie: "02".repeat(16),
                screen_number: 0,
            }),
            ..Default::default()
        },
        event_bus: None,
        pinned_test_key: Some(public_key),
    };
    let mut session = client.connect().await.unwrap();
    assert_eq!(session.host_identity.host, "127.0.0.1");
    assert_eq!(session.host_identity.port, port);
    assert_eq!(session.host_identity.username, "test");
    assert_eq!(
        session.host_identity.key_fingerprint,
        client
            .pinned_test_key
            .as_ref()
            .unwrap()
            .fingerprint(HashAlg::Sha256)
            .to_string()
    );
    session.open_shell().await.unwrap();
    let closed_before = roles
        .lock()
        .iter()
        .filter(|(_, role)| *role == "closed")
        .count();
    for _ in 0..5 {
        assert!(session
            .execute_command_output("never-exit", Duration::from_millis(100))
            .await
            .is_err());
    }
    timeout(Duration::from_secs(5), async {
        while roles
            .lock()
            .iter()
            .filter(|(_, role)| *role == "closed")
            .count()
            < closed_before + 5
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed-out exec channels must be explicitly closed");

    let language = session
        .start_language_server(LanguageServerConfig {
            executable: "aetherdesk-test-lsp".into(),
            arguments: vec![],
            working_directory: None,
        })
        .await
        .unwrap();
    let message = serde_json::json!({"jsonrpc":"2.0", "method":"test/ping", "params":{"text":"ñ"}});
    language.send(message.clone()).unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            let received = language.poll();
            if let Some(first) = received["messages"].as_array().unwrap().first() {
                assert_eq!(first, &message);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    language.stop();
    let media = session.open_bulk_loopback_channel(1).await.unwrap();
    let mut input = session
        .open_remote_loopback_channel(2)
        .await
        .unwrap()
        .into_stream();
    timeout(Duration::from_secs(10), async {
        while sent.load(Ordering::Relaxed) < 4 * 1024 * 1024 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fill the real encrypted SSH channel's receive queue");
    // Establish that the graphics connection really is blocked, not merely busy.
    assert!(timeout(
        Duration::from_millis(150),
        session.bulk.as_ref().unwrap().execute_command("probe")
    )
    .await
    .is_err());
    for _ in 0..5 {
        let started = std::time::Instant::now();
        timeout(Duration::from_millis(500), async {
            input.write_all(b"tab\n").await.unwrap();
            let mut reply = [0; 4];
            input.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"tab\n");
            assert_eq!(session.execute_command("probe").await.unwrap(), "ready\n");
            session.send_data(b"terminal-ping").unwrap();
            loop {
                if String::from_utf8_lossy(&session.read_available(65536).unwrap())
                    .contains("terminal-ping")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("graphics must not delay interactive I/O");
        eprintln!(
            "interactive round trip with blocked graphics: {:?}",
            started.elapsed()
        );
    }
    let roles = roles.lock().clone();
    let owner = |role| roles.iter().find(|(_, r)| *r == role).unwrap().0;
    assert_eq!(owner("terminal"), owner("input"));
    assert_eq!(owner("x11"), owner("media"));
    assert_ne!(owner("x11"), owner("terminal"));
    drop(media);
    drop(input);
    session.close().await.unwrap();
    let _ = stop_tx.send(());
    server.await.unwrap();
}

#[test]
fn x11_environment_is_bounded_and_shell_quoted() {
    assert!(
        x11_display_environment(b"__AETHERDESK_X11__localhost:10.0\n/home/test")
            .unwrap()
            .is_none()
    );
    assert!(x11_display_environment(b"__AETHERDESK_X11__bad;command\n/a\n").is_err());
    assert!(x11_display_environment(b"__AETHERDESK_X11__\n/a\n").is_err());
    assert_eq!(
        x11_display_environment(b"__AETHERDESK_X11__localhost:10.0\n/home/a'b/.Xauthority\n")
            .unwrap()
            .unwrap(),
        "export DISPLAY='localhost:10.0' XAUTHORITY='/home/a'\\''b/.Xauthority'"
    );
}
