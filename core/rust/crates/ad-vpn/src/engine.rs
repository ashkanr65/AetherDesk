use super::*;
use smoltcp::iface::SocketHandle;
use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::{Arc, LazyLock, Mutex, Weak};
use tokio::sync::mpsc;

type Identity = ([u8; 32], [u8; 32]);
static ENGINES: LazyLock<Mutex<HashMap<Identity, Weak<Engine>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) struct Engine {
    config: Config,
    requests: mpsc::Sender<Open>,
    stop: CancellationToken,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

struct Open {
    host: String,
    port: u16,
    local: DuplexStream,
    cancel: CancellationToken,
    ready: oneshot::Sender<Result<()>>,
}

pub(crate) async fn connect(
    config: Config,
    host: String,
    port: u16,
    cancel: CancellationToken,
) -> Result<VpnStream> {
    if host.parse().is_ok_and(|ip| !config.permits(ip)) {
        return Err(error("Host is outside WireGuard AllowedIPs"));
    }
    let identity = (
        *boringtun::x25519::PublicKey::from(&boringtun::x25519::StaticSecret::from(
            *config.private,
        ))
        .as_bytes(),
        config.public,
    );
    let engine = {
        let mut engines = ENGINES
            .lock()
            .map_err(|_| error("WireGuard session lock failed"))?;
        engines.retain(|_, entry| entry.strong_count() > 0);
        if let Some(engine) = engines
            .get(&identity)
            .and_then(Weak::upgrade)
            .filter(|e| !e.requests.is_closed())
        {
            if engine.config != config {
                return Err(error("This active WireGuard identity has different settings. Use matching settings or a separate peer key"));
            }
            engine
        } else {
            let (requests, receive) = mpsc::channel(32);
            let stop = CancellationToken::new();
            let engine = Arc::new(Engine {
                config: config.clone(),
                requests,
                stop: stop.clone(),
            });
            engines.insert(identity, Arc::downgrade(&engine));
            tokio::spawn(async move {
                let _ = stop
                    .clone()
                    .run_until_cancelled_owned(run(config, receive, stop))
                    .await;
            });
            engine
        }
    };
    let (stream, local) = tokio::io::duplex(64 * 1024);
    let cancel = cancel.child_token();
    let (ready, receive) = oneshot::channel();
    let stream = VpnStream {
        stream,
        _engine: engine.clone(),
        cancel: cancel.clone(),
    };
    let opening = async {
        engine
            .requests
            .send(Open {
                host,
                port,
                local,
                cancel: cancel.clone(),
                ready,
            })
            .await
            .map_err(|_| error("WireGuard connection stopped"))?;
        receive
            .await
            .map_err(|_| error("WireGuard connection stopped"))?
    };
    tokio::select! {
        _ = cancel.cancelled() => return Err(error("WireGuard connection cancelled")),
        result = tokio::time::timeout(Duration::from_secs(25), opening) => {
            result.map_err(|_| error("WireGuard connection timed out; check endpoint, keys and host route"))??;
        }
    }
    Ok(stream)
}

struct Flow {
    local: DuplexStream,
    cancel: CancellationToken,
    ready: Option<oneshot::Sender<Result<()>>>,
    tcp: SocketHandle,
    queries: Vec<dns::QueryHandle>,
    destination: Option<IpAddr>,
    port: u16,
    source_port: u16,
    connecting: bool,
    connected: bool,
    to_tcp: Vec<u8>,
    to_local: Vec<u8>,
}

impl Flow {
    fn open(
        request: Open,
        source_port: u16,
        config: &Config,
        iface: &mut Interface,
        sockets: &mut SocketSet<'_>,
        dns_handle: SocketHandle,
    ) -> Result<Self> {
        let destination = request.host.parse::<IpAddr>().ok();
        let mut queries = Vec::new();
        if destination.is_none() {
            if config.dns.is_empty() || config.dns.iter().any(|ip| !config.permits(*ip)) {
                let _ = request.ready.send(Err(error(
                    "Set WireGuard DNS inside AllowedIPs or use the host IP address",
                )));
                return Err(error("Invalid WireGuard DNS settings"));
            }
            let dns = sockets.get_mut::<dns::Socket>(dns_handle);
            for (family, kind) in [(true, DnsQueryType::A), (false, DnsQueryType::Aaaa)] {
                if config
                    .addresses
                    .iter()
                    .any(|a| a.addr().is_ipv4() == family)
                {
                    match dns.start_query(iface.context(), &request.host, kind) {
                        Ok(query) => queries.push(query),
                        Err(_) => {
                            for query in queries {
                                dns.cancel_query(query);
                            }
                            let _ = request.ready.send(Err(error("Invalid WireGuard DNS name")));
                            return Err(error("Invalid WireGuard DNS name"));
                        }
                    }
                }
            }
        }
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 512 * 1024]),
            tcp::SocketBuffer::new(vec![0; 64 * 1024]),
        );
        socket.set_nagle_enabled(false);
        // Interactive X11 replies must not wait for the default delayed ACK timer.
        socket.set_ack_delay(None);
        socket.set_timeout(Some(smoltcp::time::Duration::from_secs(60)));
        Ok(Self {
            local: request.local,
            cancel: request.cancel,
            ready: Some(request.ready),
            tcp: sockets.add(socket),
            queries,
            destination,
            port: request.port,
            source_port,
            connecting: false,
            connected: false,
            to_tcp: Vec::new(),
            to_local: Vec::new(),
        })
    }

    fn advance(
        &mut self,
        config: &Config,
        iface: &mut Interface,
        sockets: &mut SocketSet<'_>,
        dns_handle: SocketHandle,
    ) -> Result<bool> {
        if self.cancel.is_cancelled() {
            return Ok(false);
        }
        if self.destination.is_none() {
            let dns = sockets.get_mut::<dns::Socket>(dns_handle);
            let mut pending = Vec::new();
            for query in self.queries.drain(..) {
                match dns.get_query_result(query) {
                    Ok(addresses) => {
                        self.destination = self.destination.or_else(|| {
                            addresses
                                .into_iter()
                                .map(IpAddr::from)
                                .find(|ip| config.permits(*ip))
                        });
                    }
                    Err(dns::GetQueryResultError::Pending) => pending.push(query),
                    Err(_) => {}
                }
            }
            self.queries = pending;
            if self.destination.is_none() && self.queries.is_empty() {
                return Err(error(
                    "WireGuard DNS lookup failed or returned a host outside AllowedIPs",
                ));
            }
        }
        let socket = sockets.get_mut::<tcp::Socket>(self.tcp);
        if let Some(ip) = self.destination {
            if !self.connecting {
                if !config.permits(ip) {
                    return Err(error("Host is outside WireGuard AllowedIPs"));
                }
                socket
                    .connect(
                        iface.context(),
                        (IpAddress::from(ip), self.port),
                        self.source_port,
                    )
                    .map_err(|_| {
                        error("WireGuard has no interface address for the host IP family")
                    })?;
                self.connecting = true;
            }
        }
        if socket.state() == tcp::State::Established && !self.connected {
            self.connected = true;
            if let Some(ready) = self.ready.take() {
                let _ = ready.send(Ok(()));
            }
        }
        if self.connected
            && (!socket.is_active() || (!socket.may_recv() && self.to_local.is_empty()))
        {
            return Ok(false);
        }
        if socket.can_send() && !self.to_tcp.is_empty() {
            let n = socket
                .send_slice(&self.to_tcp)
                .map_err(|_| error("WireGuard TCP send failed"))?;
            self.to_tcp.drain(..n);
        }
        if socket.can_recv() && self.to_local.is_empty() {
            socket
                .recv(|bytes| {
                    // A paused tab holds only its own bounded buffer.
                    let n = bytes.len().min(16384);
                    self.to_local.extend_from_slice(&bytes[..n]);
                    (n, ())
                })
                .map_err(|_| error("WireGuard TCP receive failed"))?;
        }
        Ok(true)
    }

    fn poll_io(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut progress = false;
        if self.connected && self.to_tcp.is_empty() {
            let mut bytes = [0; 16384];
            let mut buffer = ReadBuf::new(&mut bytes);
            match Pin::new(&mut self.local).poll_read(cx, &mut buffer) {
                Poll::Ready(Ok(())) if !buffer.filled().is_empty() => {
                    self.to_tcp.extend_from_slice(buffer.filled());
                    progress = true;
                }
                Poll::Ready(_) => {
                    self.cancel.cancel();
                    progress = true;
                }
                Poll::Pending => {}
            }
        }
        if !self.to_local.is_empty() {
            match Pin::new(&mut self.local).poll_write(cx, &self.to_local) {
                Poll::Ready(Ok(n)) if n > 0 => {
                    self.to_local.drain(..n);
                    progress = true;
                }
                Poll::Ready(_) => {
                    self.cancel.cancel();
                    progress = true;
                }
                Poll::Pending => {}
            }
        }
        if progress {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn remove(
        mut self,
        sockets: &mut SocketSet<'_>,
        dns_handle: SocketHandle,
        result: Result<()>,
    ) -> SocketHandle {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(result);
        }
        for query in self.queries {
            sockets
                .get_mut::<dns::Socket>(dns_handle)
                .cancel_query(query);
        }
        sockets.get_mut::<tcp::Socket>(self.tcp).abort();
        self.tcp
    }
}

async fn run(
    config: Config,
    mut requests: mpsc::Receiver<Open>,
    stop: CancellationToken,
) -> Result<()> {
    let peer = tokio::net::lookup_host(&config.endpoint)
        .await?
        .next()
        .ok_or_else(|| error("WireGuard endpoint has no address"))?;
    let udp = UdpSocket::bind(if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await?;
    udp.connect(peer).await?;
    let mut tunnel = Tunn::new(
        (*config.private).into(),
        config.public.into(),
        config.preshared.as_ref().map(|key| **key),
        config.keepalive,
        rand::random::<u32>() >> 8,
        None,
    );
    let mut device = IpDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
        mtu: config.mtu,
    };
    let mut iface = stack(&config, &mut device)?;
    let mut sockets = SocketSet::new(vec![]);
    let servers: Vec<IpAddress> = config.dns.iter().copied().map(Into::into).collect();
    let dns = sockets.add(dns::Socket::new(&servers, vec![]));
    let mut flows: Vec<Flow> = Vec::new();
    let mut next_port = 49152 + rand::random::<u16>() % 8192;
    let start = std::time::Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut net = vec![0; 65536];
    let mut encoded = vec![0; 65536];
    loop {
        let now = Instant::from_millis(start.elapsed().as_millis() as i64);
        iface.poll(now, &mut device, &mut sockets);
        let mut index = 0;
        let mut closed = Vec::new();
        while index < flows.len() {
            match flows[index].advance(&config, &mut iface, &mut sockets, dns) {
                Ok(true) => index += 1,
                result => closed.push(flows.remove(index).remove(
                    &mut sockets,
                    dns,
                    result.map(|_| ()),
                )),
            }
        }
        // Flush new application bytes/ACKs now, not after the next 2 ms tick.
        iface.poll(now, &mut device, &mut sockets);
        for handle in closed {
            sockets.remove(handle);
        }
        while let Some(packet) = device.tx.pop_front() {
            let ip = match packet.first().map(|b| b >> 4) {
                Some(4) if packet.len() >= 20 => IpAddr::V4(std::net::Ipv4Addr::new(
                    packet[16], packet[17], packet[18], packet[19],
                )),
                Some(6) if packet.len() >= 40 => IpAddr::V6(std::net::Ipv6Addr::from(
                    <[u8; 16]>::try_from(&packet[24..40]).unwrap(),
                )),
                _ => continue,
            };
            if !config.permits(ip) {
                return Err(error("WireGuard refused a packet outside AllowedIPs"));
            }
            deliver(
                tunnel.encapsulate(&packet, &mut encoded),
                &udp,
                &mut device,
                &config,
            )
            .await?;
        }
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            request = requests.recv() => {
                let Some(request) = request else { return Ok(()) };
                if flows.len() >= 128 {
                    let _ = request.ready.send(Err(error("Too many active WireGuard connections")));
                    continue;
                }
                while flows.iter().any(|f| f.source_port == next_port) {
                    next_port = if next_port == 65535 { 49152 } else { next_port + 1 };
                }
                if let Ok(flow) = Flow::open(request, next_port, &config, &mut iface, &mut sockets, dns) { flows.push(flow); }
                next_port = if next_port == 65535 { 49152 } else { next_port + 1 };
            }
            result = udp.recv(&mut net) => {
                let n = result?;
                let mut more = deliver(tunnel.decapsulate(Some(peer.ip()), &net[..n], &mut encoded), &udp, &mut device, &config).await?;
                while more { more = deliver(tunnel.decapsulate(None, &[], &mut encoded), &udp, &mut device, &config).await?; }
            }
            _ = poll_fn(|cx| {
                let mut progress = false;
                for flow in &mut flows { progress |= flow.poll_io(cx).is_ready(); }
                if progress { Poll::Ready(()) } else { Poll::Pending }
            }) => {},
            _ = tick.tick() => { deliver(tunnel.update_timers(&mut encoded), &udp, &mut device, &config).await?; }
        }
    }
}
