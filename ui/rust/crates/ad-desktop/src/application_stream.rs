//! Independent outbound input and inbound lossless pictures on one SSH channel.
use super::*;
struct Abort(tokio::task::AbortHandle);
impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn open(
    target: &Target,
) -> Result<impl AsyncRead + AsyncWrite + Unpin + Send + 'static, String> {
    let channel = tokio::time::timeout(
        Duration::from_secs(10),
        target.ssh.open_bulk_exec_channel(&target.command()?),
    )
    .await
    .map_err(|_| "Application connection timed out")?
    .map_err(|e| e.to_string())?;
    let mut stream = channel.into_stream();
    match tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
        .await
        .map_err(|_| "Application helper did not start")??
    {
        Reply::Ready => Ok(stream),
        Reply::Error(error) => Err(error),
        _ => Err("Application helper did not negotiate the expected version".into()),
    }
}
pub(super) async fn run(
    target: Target,
    mut commands: mpsc::Receiver<Request>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
) -> Result<(), String> {
    let mut stream = open(&target).await?;
    write(&mut stream, &Request::Stream { active: true }).await?;
    let negotiated = tokio::time::timeout(Duration::from_secs(3), read(&mut stream)).await;
    if !matches!(negotiated, Ok(Ok(Reply::Ack))) {
        // Appended requests are unknown to older retained runtimes. Reopen a
        // clean legacy channel; never reuse a partially read negotiation.
        drop(stream);
        return legacy(open(&target).await?, &mut commands, state, ctx).await;
    }
    write(&mut stream, &Request::Titlebar { enabled: true }).await?;
    let titlebar = tokio::time::timeout(Duration::from_secs(3), read(&mut stream)).await;
    if !matches!(titlebar, Ok(Ok(Reply::Ack))) {
        // Older retained agents know streaming but not host decorations. Keep
        // the legacy view/close semantics instead of sending unknown actions.
        drop(stream);
        return legacy(open(&target).await?, &mut commands, state, ctx).await;
    }
    {
        let mut s = state.lock();
        s.streaming = true;
        s.host_titlebar = true;
    }
    let (mut receive, send) = tokio::io::split(stream);
    let writer = tokio::spawn(outbound(send, commands));
    let _guard = Abort(writer.abort_handle());
    tokio::pin!(writer);
    loop {
        tokio::select! {
            result=&mut writer=>return result.map_err(|_|"Application input worker stopped")?,
            reply=tokio::time::timeout(Duration::from_secs(15),read(&mut receive))=>{
                let reply=reply.map_err(|_|"Application update connection timed out")??;
                if frames::accept(&state,reply,None)?{ctx.request_repaint();}
            }
        }
    }
}
async fn outbound<S: AsyncWrite + Unpin>(
    mut stream: S,
    mut commands: mpsc::Receiver<Request>,
) -> Result<(), String> {
    let mut active = true;
    let mut selected = false;
    let mut pending = None;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    heartbeat.tick().await;
    let mut lists = tokio::time::interval(Duration::from_secs(1));
    loop {
        let request = if let Some(r) = pending.take() {
            r
        } else {
            tokio::select! {
                biased;
                request=commands.recv()=>request.unwrap_or(Request::Stop),
                _=lists.tick(),if !selected=>Request::List,
                _=heartbeat.tick()=>Request::Stream{active},
            }
        };
        if let Request::Stream { active: next } = &request {
            active = *next;
        }
        if matches!(&request, Request::Select(_)) {
            selected = true;
        }
        let request = batch_commands(request, &mut commands, &mut pending);
        let stop = matches!(&request, Request::Stop);
        tokio::time::timeout(Duration::from_secs(10), write(&mut stream, &request))
            .await
            .map_err(|_| "Application input write timed out")??;
        if stop {
            return Ok(());
        }
    }
}
async fn legacy<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    commands: &mut mpsc::Receiver<Request>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
) -> Result<(), String> {
    let mut ticker = tokio::time::interval(Duration::from_millis(50));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut list_due = Instant::now() - Duration::from_secs(2);
    let mut pending = None;
    loop {
        let request = if let Some(r) = pending.take() {
            r
        } else {
            tokio::select! {
                biased;
                command=commands.recv()=>command.unwrap_or(Request::Stop),
                _=ticker.tick()=>if state.lock().selected.is_some(){Request::Poll}else if list_due.elapsed()>Duration::from_secs(1){list_due=Instant::now();Request::List}else{continue;},
            }
        };
        if let Request::Stream { active } = &request {
            ticker = tokio::time::interval(Duration::from_millis(if *active { 16 } else { 125 }));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            continue;
        }
        let request = batch_commands(request, commands, &mut pending);
        let stop = matches!(&request, Request::Stop);
        tokio::time::timeout(Duration::from_secs(10), write(&mut stream, &request))
            .await
            .map_err(|_| "Application command timed out")??;
        let reply = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
            .await
            .map_err(|_| "Application capture timed out")??;
        if frames::accept(&state, reply, Some(&request))? {
            ctx.request_repaint();
        }
        if stop {
            let _ = stream.shutdown().await;
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ordered_input_does_not_wait_for_the_picture_reader() {
        let (client, mut peer) = tokio::io::duplex(256);
        let (tx, rx) = mpsc::channel(16);
        let writer = tokio::spawn(outbound(client, rx));
        for request in [
            Request::Key {
                evdev: 42,
                pressed: true,
            },
            Request::Button {
                button: 1,
                pressed: true,
                x: 3,
                y: 4,
            },
            Request::Button {
                button: 1,
                pressed: false,
                x: 3,
                y: 4,
            },
            Request::Key {
                evdev: 42,
                pressed: false,
            },
            Request::Stop,
        ] {
            tx.send(request).await.unwrap();
        }
        // Deliberately never send a single reply/image to the outbound worker.
        let mut collected = Vec::new();
        loop {
            let mut header = [0; 12];
            tokio::time::timeout(Duration::from_secs(1), peer.read_exact(&mut header))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&header[..8], wire::MAGIC);
            let mut payload = vec![0; u32::from_be_bytes(header[8..].try_into().unwrap()) as usize];
            peer.read_exact(&mut payload).await.unwrap();
            let request: Request = wire::decode(&payload).unwrap();
            match request {
                Request::InputBatch(batch) => collected.extend(batch),
                Request::Stop => break,
                Request::List => {}
                r => collected.push(wire::WindowInput::try_from(r).unwrap()),
            }
        }
        assert!(matches!(
            collected.as_slice(),
            [
                wire::WindowInput::Key {
                    evdev: 42,
                    pressed: true
                },
                wire::WindowInput::Button { pressed: true, .. },
                wire::WindowInput::Button { pressed: false, .. },
                wire::WindowInput::Key {
                    evdev: 42,
                    pressed: false
                }
            ]
        ));
        writer.await.unwrap().unwrap();
    }
}
