use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use boringtun::x25519::{PublicKey, StaticSecret};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

fn config(private: [u8; 32], public: [u8; 32], address: &str, endpoint: &str) -> Config {
    Config::parse(&format!("[Interface]\nPrivateKey={}\nAddress={address}/32\nDNS=10.8.0.1\n[Peer]\nPublicKey={}\nAllowedIPs=10.8.0.0/24\nEndpoint={endpoint}\n",STANDARD.encode(private),STANDARD.encode(public))).unwrap()
}

// A second WireGuard peer with its own TCP/IP stack. No OS tunnel or test host.
async fn peer(udp: UdpSocket, config: Config) {
    let mut tunnel = Tunn::new(
        (*config.private).into(),
        config.public.into(),
        None,
        None,
        77,
        None,
    );
    let mut device = IpDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
        mtu: 1380,
    };
    let mut iface = stack(&config, &mut device).unwrap();
    let mut sockets = SocketSet::new(vec![]);
    let mut tcp_handles = Vec::new();
    for _ in 0..8 {
        let mut tcp_socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 512 * 1024]),
            tcp::SocketBuffer::new(vec![0; 512 * 1024]),
        );
        tcp_socket.listen(2222).unwrap();
        tcp_socket.set_nagle_enabled(false);
        tcp_socket.set_ack_delay(None);
        tcp_handles.push((sockets.add(tcp_socket), Vec::new()));
    }
    use smoltcp::socket::udp as ip_udp;
    let mut dns_socket = ip_udp::Socket::new(
        ip_udp::PacketBuffer::new(vec![ip_udp::PacketMetadata::EMPTY; 8], vec![0; 8192]),
        ip_udp::PacketBuffer::new(vec![ip_udp::PacketMetadata::EMPTY; 8], vec![0; 8192]),
    );
    dns_socket.bind(53).unwrap();
    let dns_handle = sockets.add(dns_socket);
    let start = std::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(2));
    let mut buffer = vec![0u8; 65536];
    let mut encoded = vec![0u8; 65536];
    let mut remote = None;
    loop {
        iface.poll(
            Instant::from_millis(start.elapsed().as_millis() as i64),
            &mut device,
            &mut sockets,
        );
        for (handle, echo) in &mut tcp_handles {
            let socket = sockets.get_mut::<tcp::Socket>(*handle);
            if socket.can_recv() && echo.is_empty() {
                socket
                    .recv(|bytes| {
                        echo.extend_from_slice(bytes);
                        (bytes.len(), ())
                    })
                    .unwrap();
            }
            if socket.can_send() && !echo.is_empty() {
                let n = socket.send_slice(echo).unwrap();
                echo.drain(..n);
            }
        }
        let dns = sockets.get_mut::<ip_udp::Socket>(dns_handle);
        if dns.can_recv() {
            let (request, meta) = dns.recv().unwrap();
            let mut answer = request.to_vec();
            answer[2] = 0x81;
            answer[3] = 0x80;
            answer[6] = 0;
            answer[7] = 1;
            answer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 10, 8, 0, 1]);
            dns.send_slice(&answer, meta).unwrap();
        }
        if remote.is_some() {
            while let Some(packet) = device.tx.pop_front() {
                deliver(
                    tunnel.encapsulate(&packet, &mut encoded),
                    &udp,
                    &mut device,
                    &config,
                )
                .await
                .unwrap();
            }
        }
        tokio::select! {
            result=udp.recv_from(&mut buffer) => {
                let Ok((len,addr))=result else { return; };
                if remote.is_none() {udp.connect(addr).await.unwrap();remote=Some(addr);}
                let mut more=deliver(tunnel.decapsulate(Some(addr.ip()),&buffer[..len],&mut encoded),&udp,&mut device,&config).await.unwrap();
                while more {more=deliver(tunnel.decapsulate(None,&[],&mut encoded),&udp,&mut device,&config).await.unwrap();}
            }
            _=tick.tick() => {
                if remote.is_some() {deliver(tunnel.update_timers(&mut encoded),&udp,&mut device,&config).await.unwrap();}
            }
        }
    }
}

async fn connection(host: &str) -> (VpnStream, CancellationToken, JoinHandle<()>, Config) {
    let client_secret = rand::random::<[u8; 32]>();
    let server_secret = [9u8; 32];
    let client_public = *PublicKey::from(&StaticSecret::from(client_secret)).as_bytes();
    let server_public = *PublicKey::from(&StaticSecret::from(server_secret)).as_bytes();
    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let endpoint = udp.local_addr().unwrap().to_string();
    let peer = tokio::spawn(peer(
        udp,
        config(server_secret, client_public, "10.8.0.1", "127.0.0.1:1"),
    ));
    let cancel = CancellationToken::new();
    let cfg = config(client_secret, server_public, "10.8.0.2", &endpoint);
    let stream = connect_config(cfg.clone(), host.to_owned(), 2222, cancel.clone())
        .await
        .unwrap();
    (stream, cancel, peer, cfg)
}

#[tokio::test]
async fn encrypted_tcp_and_tunnel_dns_transfer_then_release_on_disconnect() {
    for host in ["10.8.0.1", "eda.internal"] {
        let (mut stream, cancel, peer, _) = connection(host).await;
        let expected: Vec<u8> = (0..1024 * 1024).map(|i| (i * 17) as u8).collect();
        let (mut read, mut write) = tokio::io::split(&mut stream);
        let mut actual = vec![0; expected.len()];
        tokio::time::timeout(Duration::from_secs(15), async {
            tokio::try_join!(write.write_all(&expected), read.read_exact(&mut actual)).unwrap();
        })
        .await
        .unwrap();
        assert_eq!(expected, actual);
        cancel.cancel();
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let engine = std::sync::Arc::downgrade(&stream._engine);
        drop(stream);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(engine.upgrade().is_none());
        peer.abort();
    }
}

#[tokio::test]
async fn duplicate_profiles_are_isolated_and_one_close_does_not_stop_the_other() {
    let (first, cancel, peer, cfg) = connection("10.8.0.1").await;
    let mut second = connect_config(
        cfg.clone(),
        "eda.internal".into(),
        2222,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first._engine, &second._engine));
    let mut incompatible = cfg.clone();
    incompatible.mtu = 1280;
    assert!(connect_config(
        incompatible,
        "10.8.0.1".into(),
        2222,
        CancellationToken::new()
    )
    .await
    .err()
    .unwrap()
    .to_string()
    .contains("different settings"));
    let mut first = first;
    let bulk = vec![0x55; 2 * 1024 * 1024];
    let busy = tokio::spawn(async move {
        let _ = first.write_all(&bulk).await;
        first
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        for byte in 0..20 {
            second.write_all(&[byte]).await.unwrap();
            assert_eq!(second.read_u8().await.unwrap(), byte);
        }
    })
    .await
    .expect("paused bulk receiver delayed interactive connection");
    cancel.cancel();
    drop(busy.await.unwrap());
    second.write_all(b"alive").await.unwrap();
    let mut reply = [0; 5];
    tokio::time::timeout(Duration::from_secs(2), second.read_exact(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&reply, b"alive");
    let third = connect_config(cfg, "10.8.0.1".into(), 2222, CancellationToken::new())
        .await
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&second._engine, &third._engine));
    drop(second);
    drop(third);
    peer.abort();
}

#[tokio::test]
async fn outside_allowed_ips_is_refused_before_endpoint_resolution() {
    let cfg = config(
        [7; 32],
        [9; 32],
        "10.8.0.2",
        "not-a-real-endpoint.invalid:1234",
    );
    let result = connect_config(cfg, "192.168.1.5".into(), 22, CancellationToken::new()).await;
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("outside WireGuard AllowedIPs"));
}
