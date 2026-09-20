//! The loopback CONNECT proxy and the host allow-list it enforces.
//!
//! Split out of wasmtime_bridge so the FFI surface there stays a surface: this
//! module owns the listener, the policy matching and the audit trail.

use crate::locking::locked;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ============================================================================
// CONNECT-based HTTPS proxy with hostname allow/deny policy.
//
// Runs a TCP listener on 127.0.0.1:<random>, reads HTTP CONNECT requests,
// and tunnels bytes for hosts matching the policy. Non-HTTPS (port != 443)
// and non-CONNECT methods are rejected. Decisions are written to stderr and
// optionally appended as JSONL to an audit file.
// ============================================================================

#[derive(Clone, Copy, PartialEq)]
enum ProxyMode {
    Allow, // only listed hosts pass
    Deny,  // all pass except listed
}

struct ProxyPolicy {
    mode: ProxyMode,
    patterns: Vec<String>,
}

struct ProxyInstance {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

static PROXIES: Mutex<Vec<Option<ProxyInstance>>> = Mutex::new(Vec::new());

/// Match a hostname against a pattern supporting `*.example.com` subdomain wildcards.
/// `*.example.com` matches `example.com` itself and any proper subdomain, but not
/// `evilexample.com`. Matching is case-insensitive.
fn host_matches(host: &str, pattern: &str) -> bool {
    if pattern.eq_ignore_ascii_case(host) {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        if host.eq_ignore_ascii_case(suffix) {
            return true;
        }
        if host.len() > suffix.len() + 1 {
            let tail = host.get(host.len() - suffix.len() - 1..);
            if tail.is_some_and(|tail| tail.eq_ignore_ascii_case(&format!(".{}", suffix))) {
                return true;
            }
        }
    }
    false
}

fn policy_allows(policy: &ProxyPolicy, host: &str) -> bool {
    let any_match = policy.patterns.iter().any(|p| host_matches(host, p));
    match policy.mode {
        ProxyMode::Allow => any_match,
        ProxyMode::Deny => !any_match,
    }
}

fn audit_log(audit_path: &Option<String>, host: &str, port: u16, decision: &str, reason: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("[porta proxy] {} {}:{} ({})", decision, host, port, reason);
    if let Some(p) = audit_path {
        let line = format!(
            "{{\"ts\":{},\"host\":{},\"port\":{},\"decision\":{},\"reason\":{}}}\n",
            ts,
            serde_json::to_string(host).unwrap_or_else(|_| "\"\"".into()),
            port,
            serde_json::to_string(decision).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(reason).unwrap_or_else(|_| "\"\"".into()),
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(line.as_bytes());
        }
    }
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

fn handle_connection(client: TcpStream, policy: Arc<ProxyPolicy>, audit_path: Arc<Option<String>>) {
    let _ = client.set_read_timeout(Some(Duration::from_secs(30)));
    let mut client_for_write = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut reader = BufReader::new(client);

    // Read the CONNECT line.
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() || first_line.is_empty() {
        return;
    }

    // Consume remaining headers up to the blank line.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    let parts: Vec<&str> = first_line.trim().split_whitespace().collect();
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("CONNECT") {
        let _ = client_for_write.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        audit_log(&audit_path, "<invalid>", 0, "deny", "non-CONNECT method");
        return;
    }
    let target = parts[1];
    let (host, port) = match target.rfind(':') {
        Some(i) => {
            let h = &target[..i];
            let p: u16 = target[i + 1..].parse().unwrap_or(0);
            (h.to_string(), p)
        }
        None => (target.to_string(), 443),
    };

    if port != 443 {
        let _ = client_for_write.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n");
        audit_log(&audit_path, &host, port, "deny", "non-443 port");
        return;
    }

    if !policy_allows(&policy, &host) {
        let _ = client_for_write.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n");
        audit_log(&audit_path, &host, port, "deny", "policy");
        return;
    }

    let sock_addr = match (host.as_str(), port).to_socket_addrs().and_then(|mut i| {
        i.next()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no addr"))
    }) {
        Ok(a) => a,
        Err(e) => {
            let _ = client_for_write.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
            audit_log(&audit_path, &host, port, "error", &format!("resolve failed: {}", e));
            return;
        }
    };
    let upstream = match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(10)) {
        Ok(s) => s,
        Err(e) => {
            let _ = client_for_write.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
            audit_log(&audit_path, &host, port, "error", &format!("upstream connect failed: {}", e));
            return;
        }
    };

    let _ = client_for_write.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
    audit_log(&audit_path, &host, port, "allow", "policy match");

    let client_side = match client_for_write.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let upstream_side = match upstream.try_clone() {
        Ok(u) => u,
        Err(_) => return,
    };
    let t1 = thread::spawn(move || { let _ = copy_bytes(client_side, upstream); });
    let t2 = thread::spawn(move || { let _ = copy_bytes(upstream_side, client_for_write); });
    let _ = t1.join();
    let _ = t2.join();
}

/// Start the CONNECT proxy on 127.0.0.1:<random>.
/// `allow_json` and `deny_json` are JSON arrays of hostname patterns; only one
/// should be non-empty. `audit_path` is a file path for JSONL logging (empty = disabled).
/// Returns JSON: {"handle":<i64>,"port":<u16>} on success, {"error":"..."} otherwise.
pub fn wt_proxy_start(
    allow_json: impl AsRef<str>,
    deny_json: impl AsRef<str>,
    audit_path: impl AsRef<str>,
) -> String {
    let allow: Vec<String> = serde_json::from_str(allow_json.as_ref()).unwrap_or_default();
    let deny: Vec<String> = serde_json::from_str(deny_json.as_ref()).unwrap_or_default();

    let policy = if !allow.is_empty() && !deny.is_empty() {
        return "{\"error\":\"allow and deny are mutually exclusive\"}".to_string();
    } else if !allow.is_empty() {
        ProxyPolicy { mode: ProxyMode::Allow, patterns: allow }
    } else if !deny.is_empty() {
        ProxyPolicy { mode: ProxyMode::Deny, patterns: deny }
    } else {
        return "{\"error\":\"neither allow nor deny list provided\"}".to_string();
    };

    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return format!("{{\"error\":\"bind failed: {}\"}}", e),
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => return format!("{{\"error\":\"local_addr failed: {}\"}}", e),
    };
    if listener.set_nonblocking(true).is_err() {
        return "{\"error\":\"set_nonblocking failed\"}".to_string();
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let audit = if audit_path.as_ref().is_empty() {
        None
    } else {
        Some(audit_path.as_ref().to_string())
    };
    let policy_arc = Arc::new(policy);
    let audit_arc = Arc::new(audit);
    let shutdown_clone = shutdown.clone();

    thread::spawn(move || {
        loop {
            if shutdown_clone.load(Ordering::Relaxed) {
                break;
            }
            match listener.accept() {
                Ok((conn, _addr)) => {
                    let _ = conn.set_nonblocking(false);
                    let p = policy_arc.clone();
                    let a = audit_arc.clone();
                    thread::spawn(move || handle_connection(conn, p, a));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    });

    let instance = ProxyInstance { port, shutdown };
    let handle = {
        let mut proxies = locked(&PROXIES);
        proxies.push(Some(instance));
        (proxies.len() - 1) as i64
    };
    format!("{{\"handle\":{},\"port\":{}}}", handle, port)
}

/// Stop the proxy associated with this handle. Returns 0 on success, -1 otherwise.
pub fn wt_proxy_stop(handle: i64) -> i64 {
    let mut proxies = locked(&PROXIES);
    let idx = handle as usize;
    if idx >= proxies.len() {
        return -1;
    }
    if let Some(inst) = &proxies[idx] {
        inst.shutdown.store(true, Ordering::Relaxed);
    }
    proxies[idx] = None;
    0
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
