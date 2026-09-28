//! Credentials handed in as placeholders, and put on in the proxy (#37).
//!
//! A command confined by porta that needs an API key used to get the key
//! itself, in its environment, where every process it starts can read it and
//! send it anywhere its network allows. Here it gets a placeholder instead:
//! `porta-cred-NAME-<random>`, worth nothing outside this run. The real value
//! stays in porta. When the command's HTTPS request to a host the credential
//! is bound to comes through the proxy, porta terminates that TLS connection
//! with a certificate from a CA made for this run, puts the real value where
//! the placeholder is, and sends the request on over a TLS connection of its
//! own that verifies the real server. The same placeholder on its way to
//! anywhere else among the intercepted hosts is refused, and recorded.
//!
//! Only bound hosts are intercepted. Every other CONNECT is tunnelled exactly
//! as before, encrypted end to end and not read: a placeholder sent there is
//! only a random string.
//!
//! Each intercepted connection carries one request: porta asks the server for
//! `Connection: close` and closes the client's side after the response, so a
//! client sends its next request on a fresh connection. That costs a
//! handshake per request to a bound host and buys a proxy that never has to
//! find where one response ends and the next begins.

use crate::proxy_audit::{audit_log, Decision};
use crate::proxy_egress::{dial, mint_token, Dial};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where one credential may go: a host pattern (`*.example.com` allowed), the
/// port, and a path prefix ("" for any path).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: String,
}

/// One credential: the name the command sees it under, the real value, the
/// placeholder the command is given instead, and where it may be put on.
pub(crate) struct Credential {
    pub(crate) name: String,
    value: String,
    pub(crate) placeholder: String,
    pub(crate) bindings: Vec<Binding>,
}

/// `host[:port][/path]` as `--credential` and `[[credentials]]` write it. The
/// proxy tunnels port 443 alone, so a binding to another port could never be
/// used, and is refused here rather than silently doing nothing.
pub(crate) fn parse_binding(text: &str) -> Result<Binding, String> {
    let text = text.trim();
    let (authority, path) = match text.find('/') {
        Some(slash) => (&text[..slash], &text[slash..]),
        None => (text, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| format!("credential binding {text}: the port is not a number"))?),
        None => (authority, 443),
    };
    if host.is_empty() {
        return Err(format!("credential binding {text}: no host"));
    }
    if port != 443 {
        return Err(format!("credential binding {text}: the proxy carries HTTPS on port 443 only"));
    }
    Ok(Binding { host: host.to_ascii_lowercase(), port, path: path.trim_end_matches('/').to_string() })
}

impl Binding {
    fn covers(&self, host: &str, port: u16, path: &str) -> bool {
        crate::http_proxy::host_matches(host, &self.host) && self.port == port && path_under(path, &self.path)
    }
}

/// Whether `path` (a request target, query and all) is `prefix` or below it:
/// `/v1` covers `/v1`, `/v1/messages` and `/v1?x`, not `/v10`.
fn path_under(target: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

/// What the request looks like once the placeholders in it have been dealt
/// with: every bound one replaced, or the first one that may not go here.
#[derive(Debug, PartialEq)]
pub(crate) enum Rewrite {
    /// The head to send on, and the names of the credentials put on.
    Send(String, Vec<String>),
    /// A placeholder for this credential is in a request it is not bound to.
    Refuse(String),
}

/// Replaces each credential's placeholder in the request head (request line,
/// query and headers alike) with its value, where the credential is bound to
/// this host, port and path; refuses the request when a placeholder is there
/// and it is not.
pub(crate) fn rewrite_head(credentials: &[Credential], head: &str, host: &str, port: u16, target: &str) -> Rewrite {
    let mut out = head.to_string();
    let mut used = Vec::new();
    for credential in credentials {
        if !out.contains(&credential.placeholder) {
            continue;
        }
        if !credential.bindings.iter().any(|binding| binding.covers(host, port, target)) {
            return Rewrite::Refuse(credential.name.clone());
        }
        out = out.replace(&credential.placeholder, &credential.value);
        used.push(credential.name.clone());
    }
    Rewrite::Send(out, used)
}

/// The CA made for one run, and the leaf certificates it has signed so far.
pub(crate) struct Broker {
    pub(crate) credentials: Vec<Credential>,
    ca: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    leaves: Mutex<HashMap<String, Arc<rustls::ServerConfig>>>,
    upstream: Arc<rustls::ClientConfig>,
    /// The directory holding the CA certificate and the trust bundle, removed
    /// when the proxy stops.
    pub(crate) dir: String,
    pub(crate) ca_path: String,
    pub(crate) bundle_path: Option<String>,
}

/// One credential as the caller hands it over: the value read from where the
/// configuration said, and the bindings as written.
#[derive(serde::Deserialize)]
pub(crate) struct Requested {
    name: String,
    value: String,
    hosts: Vec<String>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

impl Broker {
    /// A broker for these credentials: placeholders minted, the run's CA made
    /// and written out with a trust bundle the command is pointed at.
    pub(crate) fn new(requested: Vec<Requested>) -> Result<Broker, String> {
        let credentials = merged(requested)?;
        let (ca, ca_key) = make_ca()?;
        let dir = format!("/tmp/porta-credentials-{}", mint_token());
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
        let ca_path = format!("{dir}/ca.pem");
        std::fs::write(&ca_path, ca.pem()).map_err(|e| format!("cannot write {ca_path}: {e}"))?;
        // SSL_CERT_FILE and the variables like it replace the trust store
        // rather than add to it, so what they point at has to hold the
        // system's roots as well, or every other host the command reaches
        // would stop verifying.
        let bundle_path = match system_roots() {
            Some(roots) => {
                let path = format!("{dir}/bundle.pem");
                let mut bundle = roots;
                if !bundle.ends_with('\n') {
                    bundle.push('\n');
                }
                bundle.push_str(&ca.pem());
                std::fs::write(&path, bundle).map_err(|e| format!("cannot write {path}: {e}"))?;
                Some(path)
            }
            None => None,
        };
        Ok(Broker { credentials, ca, ca_key, leaves: Mutex::new(HashMap::new()), upstream: upstream_config()?, dir, ca_path, bundle_path })
    }

    /// Whether any credential is bound to this host and port, so its
    /// connections are opened rather than tunnelled.
    pub(crate) fn intercepts(&self, host: &str, port: u16) -> bool {
        self.credentials.iter().any(|c| c.bindings.iter().any(|b| crate::http_proxy::host_matches(host, &b.host) && b.port == port))
    }

    /// The hosts to add to an allow-list: a credential bound to a host the
    /// list does not reach would be handed in for nothing.
    pub(crate) fn hosts(&self) -> Vec<String> {
        self.credentials.iter().flat_map(|c| c.bindings.iter().map(|b| b.host.clone())).collect()
    }

    /// What the command's environment gains: each credential's placeholder
    /// under its name, and the variables that make its TLS clients trust the
    /// run's CA.
    pub(crate) fn child_env(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self.credentials.iter().map(|c| (c.name.clone(), c.placeholder.clone())).collect();
        // Node adds this file to its own roots; it is the one client here that
        // takes an addition rather than a replacement.
        env.push(("NODE_EXTRA_CA_CERTS".into(), self.ca_path.clone()));
        if let Some(bundle) = &self.bundle_path {
            for name in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE", "GIT_SSL_CAINFO"] {
                env.push((name.into(), bundle.clone()));
            }
        }
        env
    }

    fn leaf_for(&self, host: &str) -> Result<Arc<rustls::ServerConfig>, String> {
        let mut leaves = crate::locking::locked(&self.leaves);
        if let Some(config) = leaves.get(host) {
            return Ok(config.clone());
        }
        let config = Arc::new(self.mint_leaf(host)?);
        leaves.insert(host.to_string(), config.clone());
        Ok(config)
    }

    fn mint_leaf(&self, host: &str) -> Result<rustls::ServerConfig, String> {
        let mut params = rcgen::CertificateParams::new(vec![host.to_string()]).map_err(|e| format!("leaf for {host}: {e}"))?;
        params.distinguished_name.push(rcgen::DnType::CommonName, host);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        // Strict verifiers (Python 3.13 and later, by default) refuse a leaf
        // that does not say which key signed it.
        params.use_authority_key_identifier_extension = true;
        set_validity(&mut params);
        let key = rcgen::KeyPair::generate().map_err(|e| format!("leaf key: {e}"))?;
        let leaf = params.signed_by(&key, &self.ca, &self.ca_key).map_err(|e| format!("leaf for {host}: {e}"))?;
        let chain = vec![leaf.der().clone(), self.ca.der().clone()];
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| format!("server config: {e}"))?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(|e| format!("server config: {e}"))?;
        // HTTP/1.1 alone: the proxy reads one request head and one body, and
        // a client that would speak HTTP/2 falls back when it is not offered.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(config)
    }

    /// Takes over a CONNECT to a bound host: answers it, speaks TLS to the
    /// command as `host`, and carries one request on to the real server with
    /// the credential put on.
    pub(crate) fn intercept(&self, mut client: TcpStream, audit: &Option<String>, host: &str, port: u16) {
        let config = match self.leaf_for(host) {
            Ok(config) => config,
            Err(reason) => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                return audit_log(audit, &Decision::new(host, port, "error", reason));
            }
        };
        let _ = client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
        let Ok(connection) = rustls::ServerConnection::new(config) else { return };
        let _ = client.set_read_timeout(Some(Duration::from_secs(120)));
        let mut reader = BufReader::new(rustls::StreamOwned::new(connection, client));
        let Some((request_line, headers)) = read_head(&mut reader) else { return };
        let target = request_line.split_whitespace().nth(1).unwrap_or("/").to_string();
        let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
        let head = format!("{}\r\n{}\r\n", request_line, headers.join("\r\n"));
        match rewrite_head(&self.credentials, &head, host, port, &target) {
            Rewrite::Refuse(name) => {
                let _ = reader.get_mut().write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                close(reader.get_mut());
                audit_log(audit, &Decision::new(host, port, "deny", format!("credential {name} is not bound to {host}{path}: the request was not sent")));
            }
            Rewrite::Send(head, used) => {
                if used.is_empty() {
                    audit_log(audit, &Decision::new(host, port, "allow", format!("opened for its credentials; {path} carried none")));
                }
                for name in &used {
                    audit_log(audit, &Decision::new(host, port, "substitute", format!("credential {name} put on for {path}")));
                }
                if let Err(reason) = self.forward(&mut reader, &head, host, port) {
                    let _ = reader.get_mut().write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    audit_log(audit, &Decision::new(host, port, "error", reason));
                }
                close(reader.get_mut());
            }
        }
    }

    /// Sends the rewritten head and the body after it to the real server, then
    /// the whole response back.
    fn forward<S: Read + Write>(&self, client: &mut BufReader<S>, head: &str, host: &str, port: u16) -> Result<(), String> {
        let upstream = match dial(host, port) {
            Ok(stream) => stream,
            Err(Dial::Blocked(reason)) | Err(Dial::Failed(reason)) => return Err(reason),
        };
        let _ = upstream.set_read_timeout(Some(Duration::from_secs(300)));
        let name = ServerName::try_from(host.to_string()).map_err(|e| format!("{host}: {e}"))?;
        let connection = rustls::ClientConnection::new(self.upstream.clone(), name).map_err(|e| format!("upstream TLS: {e}"))?;
        let mut server = rustls::StreamOwned::new(connection, upstream);
        let (lines, framing) = outgoing_head(head);
        server.write_all(lines.as_bytes()).map_err(|e| format!("upstream TLS: {e}"))?;
        copy_body(client, &mut server, framing).map_err(|e| format!("request body: {e}"))?;
        server.flush().map_err(|e| format!("upstream: {e}"))?;
        let mut buffer = [0u8; 16384];
        loop {
            match server.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => client.get_mut().write_all(&buffer[..n]).map_err(|e| format!("client: {e}"))?,
                // A server that closes without close_notify has still sent the
                // whole response: it was asked for `Connection: close`.
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(format!("upstream read: {e}")),
            }
        }
        Ok(())
    }
}

/// Credentials by name: two bindings given for one name are one credential
/// with one placeholder. A name with no value is refused rather than handed
/// in as an empty key.
fn merged(requested: Vec<Requested>) -> Result<Vec<Credential>, String> {
    let mut out: Vec<Credential> = Vec::new();
    for r in requested {
        if r.value.is_empty() {
            return Err(format!("credential {}: no value to hand over (is it set in porta's environment?)", r.name));
        }
        let bindings = r.hosts.iter().map(|h| parse_binding(h)).collect::<Result<Vec<_>, _>>()?;
        match out.iter_mut().find(|c| c.name == r.name) {
            Some(existing) => existing.bindings.extend(bindings),
            None => out.push(Credential { placeholder: format!("porta-cred-{}-{}", r.name, mint_token()), name: r.name, value: r.value, bindings }),
        }
    }
    Ok(out)
}

fn make_ca() -> Result<(rcgen::Certificate, rcgen::KeyPair), String> {
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.distinguished_name.push(rcgen::DnType::CommonName, "porta run CA (this run only)");
    params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign, rcgen::KeyUsagePurpose::DigitalSignature];
    set_validity(&mut params);
    let key = rcgen::KeyPair::generate().map_err(|e| format!("CA key: {e}"))?;
    let ca = params.self_signed(&key).map_err(|e| format!("CA: {e}"))?;
    Ok((ca, key))
}

/// From yesterday to a week from now: long enough for any run, short enough
/// that a leaked certificate is soon worthless, and within the lifetimes
/// clients accept for a server certificate.
fn set_validity(params: &mut rcgen::CertificateParams) {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as i64;
    let (y1, m1, d1) = civil_from_days(days - 1);
    let (y2, m2, d2) = civil_from_days(days + 7);
    params.not_before = rcgen::date_time_ymd(y1, m1, d1);
    params.not_after = rcgen::date_time_ymd(y2, m2, d2);
}

/// Days since 1970-01-01 as a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u8, day as u8)
}

/// The system's trust store as PEM: the host's own SSL_CERT_FILE if it has
/// one, else the first bundle a distribution keeps in its usual place.
fn system_roots() -> Option<String> {
    let mut candidates: Vec<String> = std::env::var("SSL_CERT_FILE").into_iter().collect();
    candidates.extend(
        ["/etc/ssl/cert.pem", "/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem"]
            .iter()
            .map(|s| s.to_string()),
    );
    candidates.iter().find_map(|path| std::fs::read_to_string(path).ok().filter(|text| text.contains("BEGIN CERTIFICATE")))
}

/// How porta verifies the real server: the webpki roots, plus, for the
/// integration suite alone, the CA named by PORTA_TEST_UPSTREAM_CA.
fn upstream_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Ok(path) = std::env::var("PORTA_TEST_UPSTREAM_CA") {
        for cert in CertificateDer::pem_file_iter(&path).map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))? {
            let cert = cert.map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))?;
            roots.add(cert).map_err(|e| format!("PORTA_TEST_UPSTREAM_CA {path}: {e}"))?;
        }
    }
    let mut config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("client config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// How the request's body is framed, from its headers.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum Framing {
    None,
    Length(u64),
    Chunked,
}

/// The head as it goes to the server: hop-by-hop headers dropped, `Expect`
/// dropped (the body follows at once; a client waiting on 100 Continue sends
/// it after its own short wait), and `Connection: close` asked for.
pub(crate) fn outgoing_head(head: &str) -> (String, Framing) {
    let mut lines = head.split("\r\n").filter(|l| !l.is_empty());
    let request_line = lines.next().unwrap_or("");
    let mut out = format!("{request_line}\r\n");
    let mut framing = Framing::None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "connection" | "proxy-connection" | "keep-alive" | "expect" | "proxy-authorization" => continue,
            "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => framing = Framing::Chunked,
            "content-length" if framing != Framing::Chunked => framing = Framing::Length(value.trim().parse().unwrap_or(0)),
            _ => {}
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    (out, framing)
}

fn copy_body<R: BufRead, W: Write>(from: &mut R, to: &mut W, framing: Framing) -> std::io::Result<()> {
    match framing {
        Framing::None => Ok(()),
        Framing::Length(n) => {
            let copied = std::io::copy(&mut from.take(n), to)?;
            if copied < n { Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "body shorter than Content-Length")) } else { Ok(()) }
        }
        Framing::Chunked => loop {
            let mut size_line = String::new();
            if from.read_line(&mut size_line)? == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "chunked body ended early"));
            }
            to.write_all(size_line.as_bytes())?;
            let size = u64::from_str_radix(size_line.trim().split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
            if size == 0 {
                // Trailers, then the blank line that ends them.
                loop {
                    let mut line = String::new();
                    if from.read_line(&mut line)? == 0 {
                        return Ok(());
                    }
                    to.write_all(line.as_bytes())?;
                    if line == "\r\n" || line == "\n" {
                        return Ok(());
                    }
                }
            }
            std::io::copy(&mut from.take(size + 2), to)?;
        },
    }
}

/// The request line and headers, up to 64 KiB of them.
fn read_head<R: BufRead>(reader: &mut R) -> Option<(String, Vec<String>)> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut headers = Vec::new();
    let mut total = request_line.len();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return None,
            Ok(_) if line == "\r\n" || line == "\n" => return Some((request_line.trim_end().to_string(), headers)),
            Ok(n) => {
                total += n;
                if total > 65536 {
                    return None;
                }
                headers.push(line.trim_end().to_string());
            }
        }
    }
}

fn close<S: Read + Write>(stream: &mut rustls::StreamOwned<rustls::ServerConnection, S>) {
    stream.conn.send_close_notify();
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(name: &str, value: &str, hosts: &[&str]) -> Credential {
        Credential {
            name: name.into(),
            value: value.into(),
            placeholder: format!("porta-cred-{name}-0123"),
            bindings: hosts.iter().map(|h| parse_binding(h).unwrap()).collect(),
        }
    }

    #[test]
    fn bindings_parse_and_refuse_other_ports() {
        assert_eq!(parse_binding("API.example.com").unwrap(), Binding { host: "api.example.com".into(), port: 443, path: "".into() });
        assert_eq!(parse_binding("api.example.com:443/v1/").unwrap().path, "/v1");
        assert!(parse_binding("api.example.com:8443").is_err());
        assert!(parse_binding(":443").is_err());
    }

    #[test]
    fn a_placeholder_is_put_on_only_where_it_is_bound() {
        let creds = vec![credential("KEY", "real-secret", &["api.example.com/v1"])];
        let head = "GET /v1/models?k=porta-cred-KEY-0123 HTTP/1.1\r\nx-api-key: porta-cred-KEY-0123\r\n\r\n";
        match rewrite_head(&creds, head, "api.example.com", 443, "/v1/models?k=porta-cred-KEY-0123") {
            Rewrite::Send(out, used) => {
                assert!(!out.contains("porta-cred-KEY"));
                assert_eq!(out.matches("real-secret").count(), 2);
                assert_eq!(used, vec!["KEY".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rewrite_head(&creds, head, "api.example.com", 443, "/admin"), Rewrite::Refuse("KEY".into()));
        assert_eq!(rewrite_head(&creds, head, "evil.example.com", 443, "/v1/models"), Rewrite::Refuse("KEY".into()));
        assert!(!path_under("/v10", "/v1"));
    }

    #[test]
    fn a_request_without_a_placeholder_goes_as_it_is() {
        let creds = vec![credential("KEY", "real-secret", &["api.example.com"])];
        let head = "GET / HTTP/1.1\r\nHost: api.example.com\r\n\r\n";
        assert_eq!(rewrite_head(&creds, head, "api.example.com", 443, "/"), Rewrite::Send(head.into(), vec![]));
    }

    #[test]
    fn the_outgoing_head_asks_for_close_and_knows_its_body() {
        let (head, framing) = outgoing_head("POST /v1 HTTP/1.1\r\nHost: a\r\nConnection: keep-alive\r\nExpect: 100-continue\r\nContent-Length: 12\r\n\r\n");
        assert!(head.ends_with("Connection: close\r\n\r\n") && !head.contains("keep-alive") && !head.contains("Expect"));
        assert_eq!(framing, Framing::Length(12));
        assert_eq!(outgoing_head("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").1, Framing::Chunked);
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_724), (2026, 9, 28));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
