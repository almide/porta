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
//! This file holds the credentials and the rule for where each may go; `tls`
//! makes the run's CA and the certificates it signs; `exchange` carries one
//! intercepted request across.

use crate::proxy_egress::mint_token;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod exchange;
mod tls;

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

/// The host and port a CONNECT asked for.
#[derive(Clone, Copy)]
pub(crate) struct Target<'a> {
    pub(crate) host: &'a str,
    pub(crate) port: u16,
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
    fn reaches(&self, at: Target) -> bool {
        crate::http_proxy::host_matches(at.host, &self.host) && self.port == at.port
    }

    fn covers(&self, at: Target, request_target: &str) -> bool {
        self.reaches(at) && path_under(request_target, &self.path)
    }
}

/// Whether a request target (query and all) is `prefix` or below it: `/v1`
/// covers `/v1`, `/v1/messages` and `/v1?x`, not `/v10`.
fn path_under(request_target: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let path = request_target.split(['?', '#']).next().unwrap_or("");
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
pub(crate) fn rewrite_head(credentials: &[Credential], head: &str, at: Target, request_target: &str) -> Rewrite {
    let mut out = head.to_string();
    let mut used = Vec::new();
    for credential in credentials.iter().filter(|c| head.contains(&c.placeholder)) {
        if !credential.bindings.iter().any(|binding| binding.covers(at, request_target)) {
            return Rewrite::Refuse(credential.name.clone());
        }
        out = out.replace(&credential.placeholder, &credential.value);
        used.push(credential.name.clone());
    }
    Rewrite::Send(out, used)
}

/// The run's credentials, its CA, and the leaf certificates signed so far.
pub(crate) struct Broker {
    pub(crate) credentials: Vec<Credential>,
    ca: tls::Authority,
    leaves: Mutex<HashMap<String, Arc<rustls::ServerConfig>>>,
    upstream: Arc<rustls::ClientConfig>,
    /// The directory holding the CA certificate and the trust bundle, removed
    /// when the proxy stops.
    pub(crate) dir: String,
    ca_path: String,
    bundle_path: Option<String>,
}

/// One credential as the caller hands it over: the value read from where the
/// configuration said, and the bindings as written.
#[derive(serde::Deserialize)]
pub(crate) struct Requested {
    name: String,
    value: String,
    hosts: Vec<String>,
}

/// The broker a `[{"name","value","hosts"}]` list asks for, or None for an
/// empty one: a run with no credentials tunnels everything, as before.
pub(crate) fn broker_from_json(credentials_json: &str) -> Result<Option<Arc<Broker>>, String> {
    let requested: Vec<Requested> = serde_json::from_str(credentials_json).map_err(|e| format!("credentials: {e}"))?;
    if requested.is_empty() {
        return Ok(None);
    }
    Broker::new(requested).map(|broker| Some(Arc::new(broker)))
}

impl Broker {
    /// A broker for these credentials: placeholders minted, the run's CA made
    /// and written out with a trust bundle the command is pointed at.
    fn new(requested: Vec<Requested>) -> Result<Broker, String> {
        let credentials = merged(requested)?;
        let ca = tls::Authority::new()?;
        let dir = format!("/tmp/porta-credentials-{}", mint_token());
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir}: {e}"))?;
        let (ca_path, bundle_path) = ca.write_trust(&dir)?;
        Ok(Broker { credentials, ca, leaves: Mutex::new(HashMap::new()), upstream: tls::upstream_config()?, dir, ca_path, bundle_path })
    }

    /// Whether any credential is bound to this host and port, so its
    /// connections are opened rather than tunnelled.
    pub(crate) fn intercepts(&self, at: Target) -> bool {
        self.credentials.iter().any(|c| c.bindings.iter().any(|b| b.reaches(at)))
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
        let config = Arc::new(self.ca.server_config_for(host)?);
        leaves.insert(host.to_string(), config.clone());
        Ok(config)
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

#[cfg(test)]
mod tests;
