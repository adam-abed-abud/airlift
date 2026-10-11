use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// Best-effort detection of the address other LAN devices should use.
///
/// We "connect" a UDP socket to a public address. No packet is sent for UDP
/// connect; the OS merely picks the source interface for the default route,
/// which is almost always the Wi-Fi/Ethernet address a phone on the same
/// network can reach. This avoids pulling in an interface-enumeration crate
/// and works the same on Linux, macOS and Windows.
pub fn detect_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).ok()?;
    socket.connect("192.0.2.1:9").ok()?; // TEST-NET-1: never routed, never reached.
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        None
    } else {
        Some(ip)
    }
}

/// Decide which host to print in the URL. An explicit `--host` always wins;
/// a specific `--bind` address is the next best hint; otherwise auto-detect.
pub fn advertised_host(explicit: Option<&str>, bind: IpAddr) -> String {
    if let Some(h) = explicit {
        return h.to_string();
    }
    if !bind.is_unspecified() {
        return format_ip(bind);
    }
    match detect_lan_ip() {
        Some(ip) => format_ip(ip),
        None => "127.0.0.1".to_string(),
    }
}

fn format_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}
