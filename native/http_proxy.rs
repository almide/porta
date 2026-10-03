//! The loopback CONNECT proxy and the host allow-list it enforces.
//!
//! A listener on 127.0.0.1:<random> reads HTTP CONNECT requests and tunnels
//! bytes for the hosts the policy allows. A non-CONNECT method, a port other
//! than 443 and a host outside the policy are each refused, and every decision
//! is recorded through [`crate::proxy_audit`].
//!
//! Split out of wasmtime_bridge so the FFI surface there stays a surface: this
//! module owns the listener and the policy matching.

use crate::credential_broker::{broker_from_json, Broker, Target};
use crate::locking::locked;
use crate::proxy_egress::{basic_credential, mint_token, tunnel};
/// Kept reachable here, where the FFI re-exports have always found it.
pub use crate::proxy_egress::wt_is_host_allowed;
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
    /// The credentials handed in as placeholders (#37), and the CA their
    /// hosts' connections are opened with. None when the run has none: then
    /// every CONNECT is a tunnel, as it always was.
    broker: Option<Arc<Broker>>,
}

struct ProxyInstance {
    port: u16,
    shutdown: Arc<AtomicBool>,
    /// The broker's CA and trust bundle, removed when the proxy stops.
    credentials_dir: Option<String>,
}

static PROXIES: Mutex<Vec<Option<ProxyInstance>>> = Mutex::new(Vec::new());

/// Match a hostname against a pattern supporting `*.example.com` subdomain wildcards.
/// `*.example.com` matches `example.com` itself and any proper subdomain, but not
/// `evilexample.com`. Matching is case-insensitive, and one trailing dot is
/// the same name, as DNS reads it: `evil.com.` resolves where `evil.com` does,
/// and before this a deny-list naming `evil.com` let it through (found by
/// `scripts/fuzz.py proxy`).
pub(crate) fn host_matches(host: &str, pattern: &str) -> bool {
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


fn handle_connection(client: TcpStream, policy: Arc<ProxyPolicy>, audit_path: Arc<Option<String>>) {
    let _ = client.set_read_timeout(Some(Duration::from_secs(30)));
    let Ok(mut client_for_write) = client.try_clone() else { return };
    let mut reader = BufReader::new(client);
    let Some((request_line, headers)) = read_request(&mut reader) else { return };
    match connect_target(&request_line, &headers, &policy) {
        Ok((host, port)) => match &policy.broker {
            Some(broker) if broker.intercepts(Target { host: &host, port }) => broker.intercept(client_for_write, &audit_path, Target { host: &host, port }),
            _ => tunnel(client_for_write, &audit_path, &host, port),
        },
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

/// Start the CONNECT proxy on 127.0.0.1:<random>.
/// `allow_json` and `deny_json` are JSON arrays of hostname patterns; only one
/// should be non-empty. `audit_path` is a file path for JSONL logging (empty = disabled).
/// `credentials_json` is `[{"name","value","hosts":[...]}]`, the credentials to
/// hand in as placeholders; with any, the proxy may run with neither list,
/// and then every host is reachable through it.
/// Returns JSON: {"handle","port","token","env":[[name,value],...]} on success,
/// {"error":"..."} otherwise. `env` is what the command's environment gains.
pub fn wt_proxy_start(
    allow_json: impl AsRef<str>,
    deny_json: impl AsRef<str>,
    audit_path: impl AsRef<str>,
    credentials_json: impl AsRef<str>,
) -> String {
    let (broker, policy) = match proxy_policy(allow_json.as_ref(), deny_json.as_ref(), credentials_json.as_ref()) {
        Ok(ready) => ready,
        Err(reason) => return error_json(&reason),
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
    let env: Vec<(String, String)> = broker.as_ref().map(|b| b.child_env()).unwrap_or_default();
    let credentials_dir = broker.as_ref().map(|b| b.dir.clone());
    let policy = ProxyPolicy { credential: basic_credential(&token), broker, ..policy };
    serve(listener, Arc::new(policy), Arc::new(audit), shutdown.clone());

    let handle = {
        let mut proxies = locked(&PROXIES);
        proxies.push(Some(ProxyInstance { port, shutdown, credentials_dir }));
        (proxies.len() - 1) as i64
    };
    format!(
        "{{\"handle\":{},\"port\":{},\"token\":\"{}\",\"env\":{}}}",
        handle,
        port,
        token,
        serde_json::to_string(&env).unwrap_or_else(|_| "[]".into())
    )
}

fn error_json(reason: &str) -> String {
    format!("{{\"error\":{}}}", serde_json::to_string(reason).unwrap_or_else(|_| "\"\"".into()))
}

/// The broker for the run's credentials, if any, and the policy the lists ask
/// for. A credential bound to a host an allow-list leaves out would be handed
/// in for nothing; binding it says the host is meant to be reached, so it is
/// added.
fn proxy_policy(allow_json: &str, deny_json: &str, credentials_json: &str) -> Result<(Option<Arc<Broker>>, ProxyPolicy), String> {
    let broker = broker_from_json(credentials_json)?;
    let mut policy = requested_policy(allow_json, deny_json, broker.is_some()).map_err(str::to_string)?;
    if let (ProxyMode::Allow, Some(broker)) = (policy.mode, &broker) {
        policy.patterns.extend(broker.hosts());
    }
    Ok((broker, policy))
}

/// The policy these two lists ask for. At most one of them may be given: an
/// allow-list and a deny-list together have no single meaning. Neither is an
/// error unless the run hands credentials in, which is reason enough for a
/// proxy: then nothing is denied. The credential and the broker are filled in
/// by the caller.
fn requested_policy(allow_json: &str, deny_json: &str, has_credentials: bool) -> Result<ProxyPolicy, &'static str> {
    let allow: Vec<String> = serde_json::from_str(allow_json).unwrap_or_default();
    let deny: Vec<String> = serde_json::from_str(deny_json).unwrap_or_default();
    let credential = String::new();
    match (allow.is_empty(), deny.is_empty()) {
        (false, false) => Err("allow and deny are mutually exclusive"),
        (false, true) => Ok(ProxyPolicy { mode: ProxyMode::Allow, patterns: allow, credential, broker: None }),
        (true, false) => Ok(ProxyPolicy { mode: ProxyMode::Deny, patterns: deny, credential, broker: None }),
        (true, true) if has_credentials => Ok(ProxyPolicy { mode: ProxyMode::Deny, patterns: vec![], credential, broker: None }),
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
        if let Some(dir) = &inst.credentials_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    proxies[idx] = None;
    0
}
