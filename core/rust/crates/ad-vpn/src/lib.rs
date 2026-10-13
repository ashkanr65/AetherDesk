//! Process-local WireGuard with isolated TCP streams over each shared peer.
//! No TUN device, administrator privilege, proxy listener or OS route changes.
mod config;
mod engine;
pub use config::Config;

use boringtun::noise::{Tunn, TunnResult};
use smoltcp::{
    iface::{Config as StackConfig, Interface, SocketSet},
    phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::{dns, tcp},
    time::Instant,
    wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr},
};
use std::{
    collections::VecDeque,
    io,
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    net::UdpSocket,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

pub type Result<T> = io::Result<T>;
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}

/// Each stream owns its cancellation; the peer closes after its last stream.
pub struct VpnStream {
    stream: DuplexStream,
    _engine: std::sync::Arc<engine::Engine>,
    cancel: CancellationToken,
}
impl Drop for VpnStream {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl AsyncRead for VpnStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for VpnStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.cancel.cancel();
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Resolve only the WireGuard endpoint outside the tunnel. Host DNS stays inside.
pub async fn connect(
    path: &str,
    host: &str,
    port: u16,
    cancel: CancellationToken,
) -> Result<VpnStream> {
    let text = Zeroizing::new(tokio::fs::read_to_string(path).await?);
    let config = Config::parse(&text)?;
    connect_config(config, host.to_string(), port, cancel).await
}

async fn connect_config(
    config: Config,
    host: String,
    port: u16,
    cancel: CancellationToken,
) -> Result<VpnStream> {
    engine::connect(config, host, port, cancel).await
}

struct IpDevice {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
    mtu: usize,
}
struct Rx(Vec<u8>);
struct Tx<'a>(&'a mut VecDeque<Vec<u8>>);
impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut b = vec![0; len];
        let r = f(&mut b);
        self.0.push_back(b);
        r
    }
}
impl Device for IpDevice {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;
    fn receive(&mut self, _: Instant) -> Option<(Rx, Tx<'_>)> {
        self.rx.pop_front().map(|b| (Rx(b), Tx(&mut self.tx)))
    }
    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        (self.tx.len() < 128).then_some(Tx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = self.mtu;
        // The queues are software buffers. A fixed device burst limit rewrites
        // TCP windows after the socket has accounted for them; let TCP's
        // congestion control manage bursts instead.
        c.max_burst_size = None;
        c
    }
}

fn stack(config: &Config, device: &mut IpDevice) -> Result<Interface> {
    let mut cfg = StackConfig::new(HardwareAddress::Ip);
    cfg.random_seed = rand::random();
    let mut iface = Interface::new(cfg, device, Instant::from_millis(0));
    for address in &config.addresses {
        let mut inserted = Ok(());
        iface.update_ip_addrs(|addrs| {
            inserted = addrs.push(IpCidr::new(address.addr().into(), address.prefix_len()));
        });
        inserted.map_err(|_| error("Too many WireGuard interface addresses"))?;
        match address.addr() {
            IpAddr::V4(ip) => {
                iface
                    .routes_mut()
                    .add_default_ipv4_route(ip)
                    .map_err(|_| error("WireGuard route capacity"))?;
            }
            IpAddr::V6(ip) => {
                iface
                    .routes_mut()
                    .add_default_ipv6_route(ip)
                    .map_err(|_| error("WireGuard route capacity"))?;
            }
        }
    }
    Ok(iface)
}

async fn deliver(
    result: TunnResult<'_>,
    udp: &UdpSocket,
    device: &mut IpDevice,
    config: &Config,
) -> Result<bool> {
    match result {
        TunnResult::WriteToNetwork(bytes) => {
            udp.send(bytes).await?;
            // A writable UDP socket can complete a whole burst without yielding.
            // Let receive/consumer tasks drain their buffers between packets;
            // otherwise localhost peers and busy runtimes lose repeated bursts.
            tokio::task::yield_now().await;
            Ok(true)
        }
        TunnResult::WriteToTunnelV4(bytes, source) => {
            if config.permits(source.into()) && device.rx.len() < 128 {
                device.rx.push_back(bytes.to_vec());
            }
            Ok(true)
        }
        TunnResult::WriteToTunnelV6(bytes, source) => {
            if config.permits(source.into()) && device.rx.len() < 128 {
                device.rx.push_back(bytes.to_vec());
            }
            Ok(true)
        }
        // Invalid/unrelated UDP datagrams do not terminate an authenticated session.
        TunnResult::Done | TunnResult::Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests;
