//! Read Windows interface data without administrative access or invoking a shell.
use super::Adapter;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::windows::io::AsRawSocket,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR},
    NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
    Networking::WinSock::*,
};

pub(super) fn active_adapters() -> io::Result<Vec<Adapter>> {
    let mut bytes = 15 * 1024u32;
    for _ in 0..3 {
        // GetAdaptersAddresses writes an aligned list and its strings and addresses into this buffer.
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let status = unsafe {
            GetAdaptersAddresses(AF_UNSPEC.into(), flags, ptr::null(), first, &mut bytes)
        };
        match status {
            ERROR_BUFFER_OVERFLOW => continue,
            ERROR_NO_DATA => return Ok(Vec::new()),
            NO_ERROR => {}
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
        let mut result = Vec::new();
        let mut next = first;
        // The linked pointers returned by Windows are valid until buffer drops.
        while let Some(adapter) = unsafe { next.as_ref() } {
            next = adapter.Next;
            if adapter.OperStatus != IfOperStatusUp {
                continue;
            }
            let name = unsafe { wide_string(adapter.FriendlyName) };
            let description = unsafe { wide_string(adapter.Description) };
            let mut current = adapter.FirstUnicastAddress;
            let mut addresses = Vec::new();
            while let Some(unicast) = unsafe { current.as_ref() } {
                current = unicast.Next;
                let addr = unicast.Address.lpSockaddr;
                if addr.is_null() {
                    continue;
                }
                unsafe {
                    match (*addr).sa_family {
                        AF_INET => addresses.push(IpAddr::V4(Ipv4Addr::from(
                            (*addr.cast::<SOCKADDR_IN>())
                                .sin_addr
                                .S_un
                                .S_addr
                                .to_ne_bytes(),
                        ))),
                        AF_INET6 => addresses.push(IpAddr::V6(Ipv6Addr::from(
                            (*addr.cast::<SOCKADDR_IN6>()).sin6_addr.u.Byte,
                        ))),
                        _ => {}
                    }
                }
            }
            result.push(Adapter {
                name,
                wireguard: description.to_ascii_lowercase().contains("wireguard"),
                addresses,
                v4_index: unsafe { adapter.Anonymous1.Anonymous.IfIndex },
                v6_index: adapter.Ipv6IfIndex,
            });
        }
        return Ok(result);
    }
    Err(io::Error::other("Windows network interfaces kept changing"))
}

unsafe fn wide_string(value: *const u16) -> String {
    if value.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(value, len) })
}

pub(super) fn bind_interface(
    socket: &tokio::net::TcpSocket,
    index: u32,
    v4: bool,
) -> io::Result<()> {
    let (level, option, value) = if v4 {
        (IPPROTO_IP, IP_UNICAST_IF, index.to_be())
    } else {
        (IPPROTO_IPV6, IPV6_UNICAST_IF, index)
    };
    let status = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            level,
            option,
            (&value as *const u32).cast(),
            4,
        )
    };
    if status == SOCKET_ERROR {
        Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
    } else {
        Ok(())
    }
}
