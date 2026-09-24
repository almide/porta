//! The loopback CONNECT proxy and the host allow-list it enforces.
//!
//! A listener on 127.0.0.1:<random> reads HTTP CONNECT requests and tunnels
//! bytes for the hosts the policy allows. A non-CONNECT method, a port other
//! than 443 and a host outside the policy are each refused, and every decision
//! is recorded through [`crate::proxy_audit`].
//!
//! Split out of wasmtime_bridge so the FFI surface there stays a surface: this
//! module owns the listener and the policy matching.

use crate::locking::locked;
use crate::proxy_egress::{basic_credential, dial, mint_token, Dial};
use crate::proxy_audit::{audit_log, Decision};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum ProxyMode {
    Allow, // only listed hosts pass
    Deny,  // all pass except listed
}

struct ProxyPolicy {
    mode: ProxyMode,
    patterns: Vec<String>,
    /// The `Proxy-Authorization` value this run's client must present. The
    /// proxy listens on loopback, which every process of the same user can
    /// reach; without a credential it would be an open relay for any of them,
    /// carrying this run's allow-list. The token is minted per run and handed
    /// to the child in its `HTTPS_PROXY`.
    credential: String,
}

struct ProxyInstance {
    port: u16,
    shutdown: Arc<AtomicBool>,
}

static PROXIES: Mutex<Vec<Option<ProxyInstance>>> = Mutex::new(Vec::new());

/// Match a hostname against a pattern supporting `*.example.com` subdomain wildcards.
/// `*.example.com` matches `example.com` itself and any proper subdomain, but not
/// `evilexample.com`. Matching is case-insensitive, and one trailing dot is
/// the same name, as DNS reads it: `evil.com.` resolves where `evil.com` does,
/// and before this a deny-list naming `evil.com` let it through (found by
/// `scripts/fuzz.py proxy`).
fn host_matches(host: &str, pattern: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let pattern = pattern.strip_suffix('.').unwrap_or(pattern);
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
    let Some((request_line, headers)) = read_request(&mut reader) else { return };
    match connect_target(&request_line, &headers, &policy) {
        Ok((host, port)) => tunnel(client_for_write, &audit_path, &host, port),
        Err((status, decision)) => {
            let _ = client_for_write.write_all(status.as_bytes());
            audit_log(&audit_path, &decision);
        }
    }
}

/// Reads the request line and the headers that follow it.
fn read_request(reader: &mut BufReader<TcpStream>) -> Option<(String, Vec<String>)> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return None;
    }
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) if line == "\r\n" || line == "\n" => return Some((request_line, headers)),
            Ok(_) => headers.push(line.trim_end().to_string()),
        }
    }
}

/// Whether the request carries this run's credential.
fn authorised(headers: &[String], credential: &str) -> bool {
    headers.iter().any(|header| {
        header
            .split_once(':')
            .is_some_and(|(name, value)| name.trim().eq_ignore_ascii_case("proxy-authorization") && value.trim() == credential)
    })
}

/// Where this request may be tunnelled, or the status line and the record for
/// why it may not: only this run's client, only CONNECT, only port 443, only
/// an allowed host.
fn connect_target(request_line: &str, headers: &[String], policy: &ProxyPolicy) -> Result<(String, u16), (&'static str, Decision)> {
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 || !parts[0].eq_ignore_ascii_case("CONNECT") {
        return Err(("HTTP/1.1 400 Bad Request\r\n\r\n", Decision::new("<invalid>", 0, "deny", "non-CONNECT method")));
    }
    let (host, port) = split_authority(parts[1]);
    if !authorised(headers, &policy.credential) {
        return Err((
            "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"porta\"\r\n\r\n",
            Decision::new(&host, port, "deny", "not this run's client: no proxy credential"),
        ));
    }
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

/// Start the CONNECT proxy on 127.0.0.1:<random>.
/// `allow_json` and `deny_json` are JSON arrays of hostname patterns; only one
/// should be non-empty. `audit_path` is a file path for JSONL logging (empty = disabled).
/// Returns JSON: {"handle":<i64>,"port":<u16>} on success, {"error":"..."} otherwise.
pub fn wt_proxy_start(
    allow_json: impl AsRef<str>,
    deny_json: impl AsRef<str>,
    audit_path: impl AsRef<str>,
) -> String {
    let policy = match requested_policy(allow_json.as_ref(), deny_json.as_ref()) {
        Ok(policy) => policy,
        Err(reason) => return format!("{{\"error\":\"{}\"}}", reason),
    };
    let listener = match loopback_listener() {
        Ok(listener) => listener,
        Err(reason) => return format!("{{\"error\":\"{}\"}}", reason),
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(e) => return format!("{{\"error\":\"local_addr failed: {}\"}}", e),
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let audit = Some(audit_path.as_ref().to_string()).filter(|path| !path.is_empty());
    let token = mint_token();
    let policy = ProxyPolicy { credential: basic_credential(&token), ..policy };
    serve(listener, Arc::new(policy), Arc::new(audit), shutdown.clone());

    let handle = {
        let mut proxies = locked(&PROXIES);
        proxies.push(Some(ProxyInstance { port, shutdown }));
        (proxies.len() - 1) as i64
    };
    format!("{{\"handle\":{},\"port\":{},\"token\":\"{}\"}}", handle, port, token)
}

/// The policy these two lists ask for. Exactly one of them must be given:
/// an allow-list and a deny-list together have no single meaning. The
/// credential is filled in by the caller once the run's token is minted.
fn requested_policy(allow_json: &str, deny_json: &str) -> Result<ProxyPolicy, &'static str> {
    let allow: Vec<String> = serde_json::from_str(allow_json).unwrap_or_default();
    let deny: Vec<String> = serde_json::from_str(deny_json).unwrap_or_default();
    let credential = String::new();
    match (allow.is_empty(), deny.is_empty()) {
        (false, false) => Err("allow and deny are mutually exclusive"),
        (false, true) => Ok(ProxyPolicy { mode: ProxyMode::Allow, patterns: allow, credential }),
        (true, false) => Ok(ProxyPolicy { mode: ProxyMode::Deny, patterns: deny, credential }),
        (true, true) => Err("neither allow nor deny list provided"),
    }
}

/// A listener on a loopback port the kernel picks. Non-blocking, so the accept
/// loop can notice a shutdown instead of parking on accept forever.
fn loopback_listener() -> Result<TcpListener, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind failed: {}", e))?;
    listener.set_nonblocking(true).map_err(|_| "set_nonblocking failed".to_string())?;
    Ok(listener)
}

/// Accepts connections until the handle is stopped, giving each its own thread.
fn serve(listener: TcpListener, policy: Arc<ProxyPolicy>, audit: Arc<Option<String>>, shutdown: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((connection, _address)) => {
                    let _ = connection.set_nonblocking(false);
                    let (policy, audit) = (policy.clone(), audit.clone());
                    thread::spawn(move || handle_connection(connection, policy, audit));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    });
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
