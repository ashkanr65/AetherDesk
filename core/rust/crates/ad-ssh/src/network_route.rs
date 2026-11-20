//! Select once for the interactive transport, then reuse that selection for bulk SSH.
use super::{ConnectionStream, SshConfig};
use ad_config::ssh_config::WireGuardMode;
use std::{
    io,
    net::{IpAddr, SocketAddr},
    path::Path,
    time::Duration,
};
use tokio::net::{TcpSocket, TcpStream};
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
#[path = "windows_adapters.rs"]
mod windows;

#[derive(Clone)]
pub(super) enum Route {
    System {
        local: SocketAddr,
        peer: SocketAddr,
        index: u32,
        label: String,
    },
    Embedded,
}
impl Route {
    pub(super) fn label(&self) -> &str {
        match self {
            Self::System { label, .. } => label,
            Self::Embedded => "AetherDesk WireGuard",
        }
    }
}
struct Adapter {
    name: String,
    wireguard: bool,
    addresses: Vec<IpAddr>,
    v4_index: u32,
    v6_index: u32,
}
impl Adapter {
    fn index(&self, ip: IpAddr) -> u32 {
        if ip.is_ipv4() {
            self.v4_index
        } else {
            self.v6_index
        }
    }
}
type Opened = (Box<dyn ConnectionStream>, Route);

#[cfg(windows)]
fn adapters() -> io::Result<Vec<Adapter>> {
    windows::active_adapters()
}
#[cfg(not(windows))]
fn adapters() -> io::Result<Vec<Adapter>> {
    Ok(Vec::new())
}

async fn matching_adapter<'a>(all: &'a [Adapter], path: &str) -> io::Result<Option<&'a Adapter>> {
    let name = Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if let Some(adapter) = all
        .iter()
        .find(|a| a.wireguard && a.name.eq_ignore_ascii_case(name))
    {
        return Ok(Some(adapter));
    }
    if !all.iter().any(|a| a.wireguard) {
        return Ok(None);
    }
    let ips = ad_vpn::interface_addresses(path).await?;
    Ok(all
        .iter()
        .find(|a| a.wireguard && a.addresses.iter().any(|ip| ips.contains(ip))))
}

fn system(stream: TcpStream, all: &[Adapter]) -> io::Result<Opened> {
    stream.set_nodelay(true)?;
    let mut local = stream.local_addr()?;
    local.set_port(0);
    let peer = stream.peer_addr()?;
    let adapter = all.iter().find(|a| a.addresses.contains(&local.ip()));
    let label = match adapter {
        Some(a) if a.wireguard => format!("Windows WireGuard ({})", a.name),
        Some(a) => format!("Windows network ({})", a.name),
        None => "System network".to_owned(),
    };
    let route = Route::System {
        local,
        peer,
        index: adapter.map_or(0, |a| a.index(peer.ip())),
        label,
    };
    Ok((Box::new(stream), route))
}

async fn bound_connect(local: SocketAddr, peer: SocketAddr, index: u32) -> io::Result<TcpStream> {
    let socket = if peer.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    #[cfg(windows)]
    if index != 0 {
        windows::bind_interface(&socket, index, peer.is_ipv4())?;
    }
    #[cfg(not(windows))]
    let _ = index;
    socket.bind(local)?;
    let stream = socket.connect(peer).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

async fn through_adapter(config: &SshConfig, adapter: &Adapter) -> io::Result<Opened> {
    let targets = tokio::net::lookup_host((config.host.as_str(), config.port)).await?;
    let mut error = io::Error::other("No compatible address for the Windows WireGuard tunnel");
    for peer in targets {
        for &ip in &adapter.addresses {
            if ip.is_ipv4() != peer.is_ipv4() {
                continue;
            }
            match bound_connect(SocketAddr::new(ip, 0), peer, adapter.index(ip)).await {
                Ok(stream) => return system(stream, std::slice::from_ref(adapter)),
                Err(e) => error = e,
            }
        }
    }
    Err(error)
}

async fn embedded(config: &SshConfig, path: &str, cancel: CancellationToken) -> io::Result<Opened> {
    Ok((
        Box::new(ad_vpn::connect(path, &config.host, config.port, cancel).await?),
        Route::Embedded,
    ))
}

pub(super) async fn open(
    config: &SshConfig,
    pinned: Option<&Route>,
    cancel: CancellationToken,
) -> io::Result<Opened> {
    let path = config
        .wireguard_config_path
        .as_deref()
        .filter(|p| !p.trim().is_empty());
    if let Some(route) = pinned {
        return match route {
            Route::System {
                local, peer, index, ..
            } => Ok((
                Box::new(bound_connect(*local, *peer, *index).await?),
                route.clone(),
            )),
            Route::Embedded => {
                embedded(
                    config,
                    path.ok_or_else(|| io::Error::other("Missing WireGuard configuration"))?,
                    cancel,
                )
                .await
            }
        };
    }
    let mode = config.wireguard_mode.unwrap_or(if path.is_some() {
        WireGuardMode::Required
    } else {
        WireGuardMode::WindowsOnly
    });
    let direct = || TcpStream::connect((config.host.as_str(), config.port));
    if mode == WireGuardMode::WindowsOnly || mode == WireGuardMode::Automatic && path.is_none() {
        return system(direct().await?, &adapters().unwrap_or_default());
    }
    let path =
        path.ok_or_else(|| io::Error::other("Require WireGuard needs a configuration file"))?;
    if mode == WireGuardMode::Automatic {
        // Keep the connected socket. SSH verifies the host key on it before authentication.
        // Only unreachable TCP/DNS can fall back, never a rejected key or password.
        if let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(3), direct()).await {
            return system(stream, &adapters().unwrap_or_default());
        }
    }
    let all = adapters()
        .map_err(|e| io::Error::other(format!("Cannot check Windows VPN adapters: {e}")))?;
    if let Some(adapter) = matching_adapter(&all, path).await? {
        return tokio::time::timeout(Duration::from_secs(5), through_adapter(config, adapter))
            .await
            .map_err(|_| {
                io::Error::other(format!(
                    "Windows WireGuard ({}) timed out reaching the SSH host",
                    adapter.name
                ))
            })?
            .map_err(|e| {
                io::Error::other(format!(
                    "Windows WireGuard ({}) could not reach the SSH host: {e}",
                    adapter.name
                ))
            });
    }
    embedded(config, path, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn automatic_reuses_the_connected_socket_and_pins_the_second_link() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = SshConfig {
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            wireguard_mode: Some(WireGuardMode::Automatic),
            wireguard_config_path: Some("unused.conf".into()),
            ..Default::default()
        };
        let (_socket, route) = open(&config, None, CancellationToken::new()).await.unwrap();
        let (_first, _) = listener.accept().await.unwrap();
        let changed = SshConfig {
            host: "unresolvable.invalid".into(),
            ..config
        };
        let (_second, route2) = open(&changed, Some(&route), CancellationToken::new())
            .await
            .unwrap();
        let (_accepted, _) = listener.accept().await.unwrap();
        assert_eq!(route.label(), route2.label());
    }

    #[tokio::test]
    async fn required_without_configuration_never_opens_a_plain_connection() {
        let config = SshConfig {
            wireguard_mode: Some(WireGuardMode::Required),
            ..Default::default()
        };
        let error = open(&config, None, CancellationToken::new())
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("configuration file"));
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Requires an active Windows WireGuard tunnel and an authorized test account"]
    async fn live_windows_wireguard_carries_both_ssh_transports() {
        let var =
            |name: &str| std::env::var(name).expect("Set the live WireGuard test environment");
        let config = SshConfig {
            host: var("AETHER_TEST_SSH_HOST"),
            username: var("AETHER_TEST_SSH_USER"),
            key_path: Some(var("AETHER_TEST_SSH_KEY")),
            wireguard_config_path: Some(var("AETHER_TEST_WG_PROFILE")),
            wireguard_mode: Some(WireGuardMode::Automatic),
            ..Default::default()
        };
        let client = super::super::SshClient::standalone(config);
        let session = client.connect_with_bulk_transport().await.unwrap();
        assert!(session.route_label().starts_with("Windows WireGuard ("));
        let bulk = session.bulk.as_ref().unwrap();
        assert_eq!(session.route_label(), bulk.route_label());
        for (wait, ssh) in [(false, bulk.as_ref()), (true, &session)] {
            let command = if wait {
                "sleep 75 && printf route-ok"
            } else {
                "printf route-ok"
            };
            let reply = ssh
                .execute_command_output(command, Duration::from_secs(90))
                .await
                .unwrap();
            assert_eq!(reply.exit_status, 0);
            assert_eq!(reply.stdout, b"route-ok");
        }
        println!(
            "Both authenticated SSH transports used {} and remained usable for 75 seconds.",
            session.route_label()
        );
        session.close().await.unwrap();
    }
}
