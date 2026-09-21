//! The two things the loopback proxy needs beyond matching a hostname: a
//! per-run credential so it serves only this run's client, and an
//! address guard so an allowed name cannot smuggle the client somewhere it
//! should not reach.
//!
//! Kept apart from `http_proxy` so that module stays the listener and the
//! policy match, and this one holds the credential and the dial.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
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
