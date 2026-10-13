use base64::{engine::general_purpose::STANDARD, Engine};
use ipnet::IpNet;
use std::net::IpAddr;
use zeroize::Zeroizing;

use crate::{error, Result};

/// Only a path is saved in the connection profile. Key material stays in memory.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub(crate) private: Zeroizing<[u8; 32]>,
    pub(crate) public: [u8; 32],
    pub(crate) preshared: Option<Zeroizing<[u8; 32]>>,
    pub(crate) addresses: Vec<IpNet>,
    pub(crate) allowed: Vec<IpNet>,
    pub(crate) dns: Vec<IpAddr>,
    pub(crate) endpoint: String,
    pub(crate) keepalive: Option<u16>,
    pub(crate) mtu: usize,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let mut section = "";
        let mut private = None;
        let mut public = None;
        let mut preshared = None;
        let mut addresses = Vec::new();
        let mut allowed = Vec::new();
        let mut dns = Vec::new();
        let mut endpoint = None;
        let mut keepalive = None;
        let mut mtu = 1380;
        let mut peers = 0;
        if text.len() > 256 * 1024 {
            return Err(error("WireGuard config is too large"));
        }
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                section = match line {
                    "[Interface]" => "Interface",
                    "[Peer]" => {
                        peers += 1;
                        "Peer"
                    }
                    _ => return Err(error("Unsupported WireGuard config section")),
                };
                if peers > 1 {
                    return Err(error("Import a WireGuard client config with one peer"));
                }
                continue;
            }
            let (name, value) = line
                .split_once('=')
                .ok_or_else(|| error("Invalid WireGuard config line"))?;
            let name = name.trim();
            let value = value.trim();
            match (section, name) {
                ("Interface", "PrivateKey") => private = Some(Zeroizing::new(key(value)?)),
                ("Interface", "Address") => {
                    for item in value.split(',') { addresses.push(item.trim().parse::<IpNet>().map_err(|_| error("Invalid WireGuard address"))?); }
                }
                ("Interface", "DNS") => {
                    for item in value.split(',') { dns.push(item.trim().parse().map_err(|_| error("WireGuard DNS must contain IP addresses"))?); }
                }
                ("Interface", "MTU") => mtu = value.parse().map_err(|_| error("Invalid WireGuard MTU"))?,
                // These configure a system interface. No system interface exists here.
                ("Interface", "ListenPort" | "Table" | "SaveConfig") => {},
                ("Peer", "PublicKey") => public = Some(key(value)?),
                ("Peer", "PresharedKey") => preshared = Some(Zeroizing::new(key(value)?)),
                ("Peer", "AllowedIPs") => {
                    for item in value.split(',') { allowed.push(item.trim().parse().map_err(|_| error("Invalid WireGuard AllowedIPs"))?); }
                }
                ("Peer", "Endpoint") => endpoint = Some(value.to_string()),
                ("Peer", "PersistentKeepalive") => {
                    let seconds: u16 = value.parse().map_err(|_| error("Invalid WireGuard keepalive"))?;
                    keepalive = (seconds != 0).then_some(seconds);
                }
                (_, "PreUp" | "PostUp" | "PreDown" | "PostDown") => return Err(error("App-only WireGuard does not run config scripts; import a client config without hooks")),
                _ => return Err(error("Unsupported WireGuard config setting")),
            }
        }
        if addresses.is_empty()
            || addresses.len() > 4
            || allowed.is_empty()
            || peers != 1
            || !(1280..=1420).contains(&mtu)
        {
            return Err(error("WireGuard requires 1–4 interface addresses, AllowedIPs, one peer and MTU 1280–1420"));
        }
        Ok(Self {
            private: private.ok_or_else(|| error("WireGuard PrivateKey is missing"))?,
            public: public.ok_or_else(|| error("WireGuard PublicKey is missing"))?,
            preshared,
            addresses,
            allowed,
            dns,
            endpoint: endpoint.ok_or_else(|| error("WireGuard Endpoint is missing"))?,
            keepalive,
            mtu,
        })
    }
    pub(crate) fn permits(&self, ip: IpAddr) -> bool {
        self.allowed.iter().any(|range| range.contains(&ip))
    }
}

fn key(value: &str) -> Result<[u8; 32]> {
    let decoded = Zeroizing::new(
        STANDARD
            .decode(value)
            .map_err(|_| error("Invalid WireGuard key encoding"))?,
    );
    decoded
        .as_slice()
        .try_into()
        .map_err(|_| error("WireGuard keys must contain 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> String {
        format!("[Interface]\nPrivateKey={}\nAddress=10.8.0.2/32, fd00::2/128\n[Peer]\nPublicKey={}\nAllowedIPs=10.0.0.0/8,fd00::/8\nEndpoint=localhost:51820\n", STANDARD.encode([1u8;32]), STANDARD.encode([2u8;32]))
    }
    #[test]
    fn config_is_fail_closed_and_never_executes_hooks() {
        let config = Config::parse(&config()).unwrap();
        assert!(config.permits("10.8.0.1".parse().unwrap()));
        assert!(!config.permits("192.168.1.1".parse().unwrap()));
        assert!(config.permits("fd00::1".parse().unwrap()));
        assert!(!config.permits("2001:db8::1".parse().unwrap()));
    }
    #[test]
    fn rejects_scripts_multiple_peers_and_invalid_keys_without_echoing_secrets() {
        assert!(Config::parse(&(config() + "\nPostUp=echo private\n")).is_err());
        assert!(Config::parse(&(config() + "\n[Peer]\n")).is_err());
        let invalid = config().replace(&STANDARD.encode([1u8; 32]), "secret-material");
        let err = Config::parse(&invalid).err().unwrap().to_string();
        assert!(!err.contains("secret-material"));
    }
}
