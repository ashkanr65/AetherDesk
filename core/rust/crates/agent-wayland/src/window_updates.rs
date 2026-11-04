//! Event-driven capture with a bounded writer independent of ordered input.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, Notify};

async fn send(output: &Pipe, reply: wire::Reply) -> Result<(), Error> {
    let bytes = tokio::task::spawn_blocking(move || wire::encode(&reply)).await??;
    tokio::time::timeout(Duration::from_secs(10), async {
        output.write_all(wire::MAGIC).await?;
        output
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await?;
        output.write_all(&bytes).await
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn protocol_fixture() {
        let Ok(session) = std::env::var("AETHERDESK_WINDOW_PROTOCOL_FIXTURE") else {
            return;
        };
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(serve_io(&session, 0, 0))
            .unwrap();
    }
    pub(super) fn verify(session: &str, controller: &RustConnection, window: u32) {
        use std::process::{Command, Stdio};
        if let Ok(baseline) = std::env::var("AETHERDESK_WINDOW_BASELINE_AGENT") {
            compare_legacy(&baseline, session, controller, window);
        }
        let (mut client, server) = UnixStream::pair().unwrap();
        let buffer = 4096i32;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&buffer as *const i32).cast(),
                    std::mem::size_of_val(&buffer) as u32,
                )
            },
            0
        );
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "window_mirror::updates::tests::protocol_fixture",
                "--nocapture",
            ])
            .env("AETHERDESK_WINDOW_PROTOCOL_FIXTURE", session)
            .stdin(Stdio::from(OwnedFd::from(server)))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        struct Cleanup<'a> {
            child: &'a mut std::process::Child,
            connection: &'a RustConnection,
            root: u32,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                if let Ok(cookie) = xtest::fake_input(
                    self.connection,
                    KEY_RELEASE_EVENT,
                    38,
                    0,
                    self.root,
                    0,
                    0,
                    0,
                ) {
                    let _ = cookie.check();
                }
            }
        }
        let guard = Cleanup {
            child: &mut child,
            connection: controller,
            root: controller.setup().roots[0].root,
        };
        fn write(client: &mut UnixStream, request: wire::Request) {
            let bytes = wire::encode(&request).unwrap();
            client.write_all(wire::MAGIC).unwrap();
            client
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .unwrap();
            client.write_all(&bytes).unwrap();
        }
        fn read(client: &mut UnixStream) -> wire::Reply {
            let mut magic = [0; 8];
            client.read_exact(&mut magic).unwrap();
            assert_eq!(&magic, wire::MAGIC);
            let mut length = [0; 4];
            client.read_exact(&mut length).unwrap();
            let size = u32::from_be_bytes(length) as usize;
            assert!(size < wire::MAX_PACKET);
            let mut bytes = vec![0; size];
            client.read_exact(&mut bytes).unwrap();
            wire::decode(&bytes).unwrap()
        }
        assert!(matches!(read(&mut client), wire::Reply::Ready));
        write(&mut client, wire::Request::Stream { active: true });
        assert!(matches!(read(&mut client), wire::Reply::Ack));
        write(&mut client, wire::Request::Select(window));
        assert!(matches!(read(&mut client), wire::Reply::Selected(_)));
        assert!(
            matches!(read(&mut client), wire::Reply::Frame { .. }),
            "initial picture needed a poll"
        );
        write(&mut client, wire::Request::Focus);
        let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
        let gc = controller.generate_id().unwrap();
        controller
            .create_gc(gc, window, &CreateGCAux::new())
            .unwrap()
            .check()
            .unwrap();
        let mut random = 0xabcdef01u32;
        let mut pixels = Vec::new();
        for _ in 0..u32::from(geometry.width) * u32::from(geometry.height) {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            pixels.extend_from_slice(&random.to_le_bytes());
        }
        controller
            .put_image(
                ImageFormat::Z_PIXMAP,
                window,
                gc,
                geometry.width,
                geometry.height,
                0,
                0,
                0,
                geometry.depth,
                &pixels,
            )
            .unwrap()
            .check()
            .unwrap();
        // Allow the image to fill a deliberately tiny output socket. The parent
        // does not drain a single image byte until key press/release both arrive.
        std::thread::sleep(Duration::from_millis(80));
        let started = Instant::now();
        write(
            &mut client,
            wire::Request::Key {
                evdev: 30,
                pressed: true,
            },
        );
        while controller.query_keymap().unwrap().reply().unwrap().keys[38 / 8] & (1 << (38 % 8))
            == 0
        {
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "input waited behind a blocked image writer"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let pressed_us = started.elapsed().as_micros();
        write(
            &mut client,
            wire::Request::Key {
                evdev: 30,
                pressed: false,
            },
        );
        while controller.query_keymap().unwrap().reply().unwrap().keys[38 / 8] & (1 << (38 % 8))
            != 0
        {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "release waited behind a blocked image writer"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            matches!(read(&mut client), wire::Reply::Frame { .. }),
            "damage did not stream a picture"
        );
        controller
            .change_property8(
                PropMode::REPLACE,
                window,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"Library Manager renamed",
            )
            .unwrap()
            .check()
            .unwrap();
        let mut renamed = false;
        for _ in 0..4 {
            if matches!(read(&mut client),wire::Reply::Metadata(info) if info.title=="Library Manager renamed")
            {
                renamed = true;
                break;
            }
        }
        assert!(renamed, "source title changes did not stream");
        write(&mut client, wire::Request::Stop);
        while !matches!(read(&mut client), wire::Reply::Ack) {}
        controller.free_gc(gc).unwrap().check().unwrap();
        eprintln!("PERF: blocked-image input press reached X11 in {pressed_us} us; release also arrived before picture drain");
        drop(client);
        assert!(guard.child.wait().unwrap().success());
    }

    // Optional developer benchmark against an explicitly supplied previous
    // package. Ordinary builds never require or launch an external baseline.
    fn compare_legacy(executable: &str, session: &str, controller: &RustConnection, window: u32) {
        use std::process::{Command, Stdio};
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let buffer = 4096i32;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&buffer as *const i32).cast(),
                    std::mem::size_of_val(&buffer) as u32,
                )
            },
            0
        );
        let mut child = Command::new(executable)
            .args(["--window-mirror", "--session-id", session])
            .env("LD_LIBRARY_PATH", "/baseline-runtime/libs")
            .stdin(Stdio::from(OwnedFd::from(server.try_clone().unwrap())))
            .stdout(Stdio::from(OwnedFd::from(server)))
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        struct Cleanup<'a>(&'a mut std::process::Child, &'a RustConnection);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
                if let Ok(cookie) = xtest::fake_input(
                    self.1,
                    KEY_RELEASE_EVENT,
                    38,
                    0,
                    self.1.setup().roots[0].root,
                    0,
                    0,
                    0,
                ) {
                    let _ = cookie.check();
                }
            }
        }
        let guard = Cleanup(&mut child, controller);
        fn write(client: &mut UnixStream, request: wire::Request) {
            let bytes = wire::encode(&request).unwrap();
            client.write_all(wire::MAGIC).unwrap();
            client
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .unwrap();
            client.write_all(&bytes).unwrap();
        }
        fn read(client: &mut UnixStream) -> wire::Reply {
            let mut header = [0; 12];
            client.read_exact(&mut header).unwrap();
            assert_eq!(&header[..8], wire::MAGIC);
            let size = u32::from_be_bytes(header[8..].try_into().unwrap()) as usize;
            assert!(size < wire::MAX_PACKET);
            let mut bytes = vec![0; size];
            client.read_exact(&mut bytes).unwrap();
            wire::decode(&bytes).unwrap()
        }
        assert!(matches!(read(&mut client), wire::Reply::Ready));
        write(&mut client, wire::Request::Select(window));
        assert!(matches!(read(&mut client), wire::Reply::Selected(_)));
        write(&mut client, wire::Request::Focus);
        assert!(matches!(read(&mut client), wire::Reply::Ack));
        write(&mut client, wire::Request::Poll);
        assert!(matches!(read(&mut client), wire::Reply::Frame { .. }));
        let geometry = controller.get_geometry(window).unwrap().reply().unwrap();
        let gc = controller.generate_id().unwrap();
        controller
            .create_gc(gc, window, &CreateGCAux::new())
            .unwrap()
            .check()
            .unwrap();
        let mut random = 0x12345678u32;
        let mut pixels = Vec::new();
        for _ in 0..u32::from(geometry.width) * u32::from(geometry.height) {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            pixels.extend_from_slice(&random.to_le_bytes());
        }
        controller
            .put_image(
                ImageFormat::Z_PIXMAP,
                window,
                gc,
                geometry.width,
                geometry.height,
                0,
                0,
                0,
                geometry.depth,
                &pixels,
            )
            .unwrap()
            .check()
            .unwrap();
        write(&mut client, wire::Request::Poll);
        std::thread::sleep(Duration::from_millis(80));
        write(
            &mut client,
            wire::Request::Key {
                evdev: 30,
                pressed: true,
            },
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(150) {
            assert_eq!(
                controller.query_keymap().unwrap().reply().unwrap().keys[38 / 8] & (1 << (38 % 8)),
                0,
                "baseline did not exhibit blocking; do not report this comparison"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(read(&mut client), wire::Reply::Frame { .. }));
        assert!(matches!(read(&mut client), wire::Reply::Ack));
        assert_ne!(
            controller.query_keymap().unwrap().reply().unwrap().keys[38 / 8] & (1 << (38 % 8)),
            0
        );
        write(
            &mut client,
            wire::Request::Key {
                evdev: 30,
                pressed: false,
            },
        );
        assert!(matches!(read(&mut client), wire::Reply::Ack));
        write(&mut client, wire::Request::Stop);
        assert!(matches!(read(&mut client), wire::Reply::Ack));
        drop(client);
        assert!(guard.0.wait().unwrap().success());
        controller.free_gc(gc).unwrap().check().unwrap();
        eprintln!("PERF: previous built agent did not deliver input during a 150 ms blocked-image interval; input arrived after draining the picture");
    }
}

#[cfg(test)]
pub(super) fn verify_stream(session: &str, controller: &RustConnection, window: u32) {
    tests::verify(session, controller, window);
}
async fn receive(input: &Pipe) -> Result<wire::Request, Error> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic).await?;
    if &magic != wire::MAGIC {
        return Err("Invalid window stream identity".into());
    }
    let mut length = [0; 4];
    input.read_exact(&mut length).await?;
    let count = u32::from_be_bytes(length) as usize;
    if count > 64 * 1024 {
        return Err("Oversized window request".into());
    }
    let mut bytes = vec![0; count];
    input.read_exact(&mut bytes).await?;
    Ok(wire::decode(&bytes)?)
}
struct Abort(tokio::task::AbortHandle);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn serve(session: &str) -> Result<(), Error> {
    serve_io(session, 0, 1).await
}
async fn serve_io(session: &str, input_fd: i32, output_fd: i32) -> Result<(), Error> {
    let input = Arc::new(Pipe::new(input_fd)?);
    let output = Arc::new(Pipe::new(output_fd)?);
    let mut mirror = match Mirror::connect_session(session) {
        Ok(value) => value,
        Err(error) => {
            send(
                &output,
                wire::Reply::Error(error.to_string().chars().take(240).collect()),
            )
            .await?;
            return Ok(());
        }
    };
    send(&output, wire::Reply::Ready).await?;
    let duplicate = unsafe { libc::dup(mirror.connection.stream().as_raw_fd()) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let events = AsyncFd::with_interest(
        unsafe { OwnedFd::from_raw_fd(duplicate) },
        tokio::io::Interest::READABLE,
    )?;
    let (tx, mut replies) = mpsc::channel::<wire::Reply>(8);
    let in_flight = Arc::new(AtomicBool::new(false));
    let drained = Arc::new(Notify::new());
    let writer = {
        let output = output.clone();
        let busy = in_flight.clone();
        let notify = drained.clone();
        tokio::spawn(async move {
            while let Some(reply) = replies.recv().await {
                let frame = matches!(&reply, wire::Reply::Frame { .. });
                send(&output, reply).await?;
                if frame {
                    busy.store(false, Ordering::Release);
                }
                notify.notify_one();
            }
            Ok::<_, Error>(())
        })
    };
    let _writer = Abort(writer.abort_handle());
    let (requests, mut rx) = mpsc::channel(128);
    let reader = tokio::spawn(async move {
        while let Ok(Ok(request)) =
            tokio::time::timeout(Duration::from_secs(15), receive(&input)).await
        {
            if requests.send(request).await.is_err() {
                break;
            }
        }
    });
    let _reader = Abort(reader.abort_handle());
    let mut streaming = false;
    let mut active = true;
    let mut pending = None;
    let mut needed = false;
    let mut last_info = None;
    let mut next = tokio::time::Instant::now();
    let mut fallback = next + Duration::from_millis(500);
    let mut heartbeat = next + Duration::from_secs(5);
    loop {
        if writer.is_finished() {
            break;
        }
        if let Some(request) = pending.take() {
            let stop = matches!(&request, wire::Request::Stop);
            let quiet = streaming
                && (wire::batchable(&request) || matches!(&request, wire::Request::InputBatch(_)));
            let clear = matches!(&request, wire::Request::Poll | wire::Request::Select(_));
            if let wire::Request::Stream { active: foreground } = request {
                streaming = true;
                active = foreground;
                needed = true;
                next = tokio::time::Instant::now();
                tx.send(wire::Reply::Ack)
                    .await
                    .map_err(|_| "Application output closed")?;
            } else {
                let reply = if streaming && matches!(&request, wire::Request::Poll) {
                    needed = true;
                    wire::Reply::Ack
                } else {
                    match mirror.handle(request) {
                        Ok(reply) => reply,
                        Err(error) => {
                            let _ = mirror.release_input();
                            if clear {
                                let _ = mirror.clear();
                            }
                            if streaming && clear {
                                wire::Reply::ViewClosed(
                                    error.to_string().chars().take(240).collect(),
                                )
                            } else {
                                input_failure_reply(&error)
                            }
                        }
                    }
                };
                if let wire::Reply::Selected(info) = &reply {
                    last_info = Some(info.clone());
                    needed = true;
                    next = tokio::time::Instant::now();
                }
                if !quiet || !matches!(&reply, wire::Reply::Ack) {
                    tx.send(reply)
                        .await
                        .map_err(|_| "Application output closed")?;
                }
            }
            if stop {
                drop(tx);
                let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
                return Ok(());
            }
        }
        let now = tokio::time::Instant::now();
        if streaming
            && mirror.selected.is_some()
            && needed
            && now >= next
            && !in_flight.load(Ordering::Acquire)
            && tx.capacity() >= 2
        {
            let reply = match mirror.frame() {
                Ok(reply) => reply,
                Err(error) => {
                    let _ = mirror.clear();
                    wire::Reply::ViewClosed(error.to_string().chars().take(240).collect())
                }
            };
            if let Some(info) = mirror.selected.as_ref().map(|s| s.info.clone()) {
                if last_info.as_ref() != Some(&info) {
                    last_info = Some(info.clone());
                    tx.try_send(wire::Reply::Metadata(info))
                        .map_err(|_| "Application output overflow")?;
                }
            }
            if !matches!(&reply, wire::Reply::Idle) {
                let frame = matches!(&reply, wire::Reply::Frame { .. });
                if frame {
                    in_flight.store(true, Ordering::Release);
                }
                tx.try_send(reply)
                    .map_err(|_| "Application output overflow")?;
            }
            needed = false;
            next = now + Duration::from_millis(if active { 16 } else { 125 });
        }
        if now >= heartbeat {
            if tx.capacity() > 0 {
                let _ = tx.try_send(wire::Reply::Idle);
            }
            heartbeat = now + Duration::from_secs(5);
        }
        if now >= fallback {
            needed = true;
            fallback = now + Duration::from_millis(if active { 500 } else { 1000 });
        }
        let capture = if streaming
            && needed
            && mirror.selected.is_some()
            && !in_flight.load(Ordering::Acquire)
            && tx.capacity() >= 2
        {
            next.max(now + Duration::from_millis(1))
        } else {
            fallback
        };
        tokio::select! {
            biased;
            request=rx.recv()=>match request{Some(request)=>pending=Some(request),None=>break},
            ready=events.readable()=>{let mut ready=ready?;mirror.events()?;ready.clear_ready();needed=true;},
            _=drained.notified()=>{},
            _=tokio::time::sleep_until(capture.min(fallback).min(heartbeat))=>{},
        }
    }
    // A stopped reader/client must release only this mirror's owned input.
    let _ = mirror.clear();
    Ok(())
}
