//! The two things the loopback proxy needs beyond matching a hostname: a
//! per-run credential so it serves only this run's client, and an
//! address guard so an allowed name cannot smuggle the client somewhere it
//! should not reach.
//!
//! Kept apart from `http_proxy` so that module stays the listener and the
//! policy match, and this one holds the credential and the dial.

use crate::proxy_audit::{audit_log, Decision};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::thread;
use std::time::Duration;

/// A fresh token for one run: 16 random bytes, as hex. A label for a client,
/// not a secret store — it lives only as long as the proxy.
pub(crate) fn mint_token() -> String {
    let mut buffer = [0u8; 16];
    let _ = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut buffer));
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `Basic` credentials for `porta:<token>`, as a client puts them on the wire.
pub(crate) fn basic_credential(token: &str) -> String {
    format!("Basic {}", base64(format!("porta:{token}").as_bytes()))
}

/// Standard base64 with padding. Twenty lines here beat a dependency for one
/// header.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let bits = chunk.iter().fold(0u32, |acc, byte| (acc << 8) | *byte as u32) << (8 * (3 - chunk.len()));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((bits >> (18 - 6 * index)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Why a dial did not happen: the host resolved only to addresses the proxy
/// refuses to reach, or it could not be reached at all.
pub(crate) enum Dial {
    Blocked(String),
    Failed(String),
}

/// Whether an address is one an allowed hostname must not be able to smuggle
/// the client to: this machine, the link, a cloud metadata endpoint, a
/// multicast group. A name on the allow-list is a promise about a public
/// service; a name that resolves here is a different thing wearing its label.
/// Private ranges stay reachable — an internal API is a legitimate target.
fn blocked_address(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => blocked_v4(v4),
        std::net::IpAddr::V6(v6) => blocked_v6(v6),
    }
}

fn blocked_v4(v4: &std::net::Ipv4Addr) -> bool {
    v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() || v4.is_broadcast() || v4.is_multicast() || v4.octets()[0] == 0
}

fn blocked_v6(v6: &std::net::Ipv6Addr) -> bool {
    if let Some(mapped) = v6.to_ipv4_mapped() {
        return blocked_v4(&mapped);
    }
    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (v6.segments()[0] & 0xffc0) == 0xfe80
        || v6.segments() == [0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254]
}

/// The first address the host resolves to that the proxy may reach,
/// connected within ten seconds. Resolved once and dialled by address, so
/// what was checked is what is connected.
pub(crate) fn dial(host: &str, port: u16) -> Result<TcpStream, Dial> {
    if let Some(address) = test_upstream(host) {
        return TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(|e| Dial::Failed(format!("upstream connect failed: {e}")));
    }
    let addresses: Vec<std::net::SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| Dial::Failed(format!("resolve failed: {e}")))?
        .collect();
    if addresses.is_empty() {
        return Err(Dial::Failed("resolve failed: no addr".to_string()));
    }
    let Some(address) = addresses.iter().find(|address| !blocked_address(&address.ip())) else {
        return Err(Dial::Blocked(format!(
            "resolves only to blocked addresses ({}): loopback, link-local, metadata and multicast are never reached by name",
            addresses.iter().map(|address| address.ip().to_string()).collect::<Vec<_>>().join(", ")
        )));
    };
    TcpStream::connect_timeout(address, Duration::from_secs(10))
        .map_err(|e| Dial::Failed(format!("upstream connect failed: {e}")))
}

/// For scripts/integration.py alone: `PORTA_TEST_UPSTREAM=host=127.0.0.1:port,...`
/// sends a CONNECT for `host` to a local test server, past the address guard,
/// so the credential broker can be exercised end to end without the internet.
/// It is read from porta's own environment, which the confined command
/// cannot set; whoever can set it already runs porta.
fn test_upstream(host: &str) -> Option<std::net::SocketAddr> {
    let spec = std::env::var("PORTA_TEST_UPSTREAM").ok()?;
    spec.split(',').find_map(|entry| {
        let (name, address) = entry.split_once('=')?;
        if name.trim().eq_ignore_ascii_case(host) { address.trim().parse().ok() } else { None }
    })
}

/// Opens the tunnel and copies bytes both ways until either side closes. An
/// unreachable upstream is answered and recorded rather than left hanging.
pub(crate) fn tunnel(mut client: TcpStream, audit_path: &Option<String>, host: &str, port: u16) {
    let upstream = match dial(host, port) {
        Ok(upstream) => upstream,
        Err(Dial::Blocked(reason)) => {
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n");
            return audit_log(audit_path, &Decision::new(host, port, "deny", reason));
        }
        Err(Dial::Failed(reason)) => {
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
            return audit_log(audit_path, &Decision::new(host, port, "error", reason));
        }
    };
    let _ = client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
    audit_log(audit_path, &Decision::new(host, port, "allow", "policy match"));
    let (Ok(client_side), Ok(upstream_side)) = (client.try_clone(), upstream.try_clone()) else { return };
    let outbound = thread::spawn(move || { let _ = copy_bytes(client_side, upstream); });
    let inbound = thread::spawn(move || { let _ = copy_bytes(upstream_side, client); });
    let _ = outbound.join();
    let _ = inbound.join();
}

fn copy_bytes(mut src: TcpStream, mut dst: TcpStream) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            let _ = dst.shutdown(std::net::Shutdown::Write);
            return Ok(());
        }
        dst.write_all(&buf[..n])?;
    }
}

/// Use the same URL parser as the HTTP transport, never a string split.
pub fn wt_is_host_allowed(url: impl AsRef<str>, allowed_json: impl AsRef<str>) -> bool {
    let Ok(url) = reqwest::Url::parse(url.as_ref()) else { return false };
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else { return false };
    let Ok(allowed) = serde_json::from_str::<Vec<String>>(allowed_json.as_ref()) else { return false };
    allowed.iter().any(|rule| {
        let Some((allowed_host, allowed_port)) = rule.rsplit_once(':') else { return false };
        (allowed_host == "*" || allowed_host.eq_ignore_ascii_case(host)) &&
            (allowed_port == "*" || allowed_port.parse::<u16>().ok() == Some(port))
    })
}
