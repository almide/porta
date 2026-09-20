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

/// One proxy decision, exactly as it is reported and recorded.
struct Decision {
    host: String,
    port: u16,
    verdict: &'static str,
    reason: String,
}

impl Decision {
    fn new(host: &str, port: u16, verdict: &'static str, reason: impl Into<String>) -> Self {
        Self { host: host.to_string(), port, verdict, reason: reason.into() }
    }
}

fn audit_log(audit_path: &Option<String>, decision: &Decision) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("[porta proxy] {} {}:{} ({})", decision.verdict, decision.host, decision.port, decision.reason);
    if let Some(p) = audit_path {
        let line = format!(
            "{{\"ts\":{},\"host\":{},\"port\":{},\"decision\":{},\"reason\":{}}}\n",
            ts,
            serde_json::to_string(&decision.host).unwrap_or_else(|_| "\"\"".into()),
            decision.port,
            serde_json::to_string(decision.verdict).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&decision.reason).unwrap_or_else(|_| "\"\"".into()),
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
    let Ok(mut client_for_write) = client.try_clone() else { return };
    let mut reader = BufReader::new(client);
    let Some(request_line) = read_request(&mut reader) else { return };
    match connect_target(&request_line, &policy) {
        Ok((host, port)) => tunnel(client_for_write, &audit_path, &host, port),
        Err((status, decision)) => {
            let _ = client_for_write.write_all(status.as_bytes());
            audit_log(&audit_path, &decision);
        }
    }
}

/// Reads the request line and drains the headers that follow it.
fn read_request(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return None;
    }
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) if line == "\r\n" || line == "\n" => return Some(request_line),
            Ok(_) => {}
        }
    }
}

/// Where this request may be tunnelled, or the status line and the record for
/// why it may not: only CONNECT, only port 443, only an allowed host.
fn connect_target(request_line: &str, policy: &ProxyPolicy) -> Result<(String, u16), (&'static str, Decision)> {
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("CONNECT") {
        return Err(("HTTP/1.1 400 Bad Request\r\n\r\n", Decision::new("<invalid>", 0, "deny", "non-CONNECT method")));
    }
    let (host, port) = split_authority(parts[1]);
    if port != 443 {
        return Err(("HTTP/1.1 403 Forbidden\r\n\r\n", Decision::new(&host, port, "deny", "non-443 port")));
    }
    if !policy_allows(policy, &host) {
        return Err(("HTTP/1.1 403 Forbidden\r\n\r\n", Decision::new(&host, port, "deny", "policy")));
    }
    Ok((host, port))
}

/// Splits `host:port`, defaulting to HTTPS when no port is given. A port that
/// is not a number becomes 0, which the port check then refuses.
fn split_authority(target: &str) -> (String, u16) {
    match target.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(0)),
        None => (target.to_string(), 443),
    }
}

/// Opens the tunnel and copies bytes both ways until either side closes. An
/// unreachable upstream is answered and recorded rather than left hanging.
fn tunnel(mut client: TcpStream, audit_path: &Option<String>, host: &str, port: u16) {
    let upstream = match dial(host, port) {
        Ok(upstream) => upstream,
        Err(reason) => {
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

/// The first address the host resolves to, connected within ten seconds.
fn dial(host: &str, port: u16) -> Result<TcpStream, String> {
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve failed: {e}"))?
        .next()
        .ok_or_else(|| "resolve failed: no addr".to_string())?;
    TcpStream::connect_timeout(&address, Duration::from_secs(10))
        .map_err(|e| format!("upstream connect failed: {e}"))
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
