//! The profile's network rules: `--no-net`, the TCP ports `--allow-net`
//! names, the ports to listen on, and the Unix sockets closed or reopened.

use super::*;

/// The socket every macOS resolver call goes through. `getaddrinfo` does not
/// send DNS itself; it asks mDNSResponder over this path, and that daemon —
/// outside the sandbox — does the lookup.
pub(super) const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// `--no-net`: no outbound, no listening, no inbound, and no resolver. The
/// Unix sockets `--allow-unix` names are reopened after this, as in every mode.
pub(super) const NO_NETWORK_RULES: &str = "(deny network-outbound)\n(deny network-bind)\n(deny network-inbound)\n";

/// The network is open like Docker's until `--allow-net` names a port, which
/// then closes everything else. Only the port is filtered, not the host.
///
/// Closing everything else also closes the resolver socket, and a command
/// that cannot resolve a name cannot use the port it was granted: `curl
/// --allow-net '*:443' https://example.com` failed with "Could not resolve
/// host" for exactly as long as this rule was missing. Reopening it gives up
/// nothing the port grant did not already give — the host part of
/// `--allow-net` is not enforced, so any address on that port was already
/// reachable, by number. Proxy mode is different: there the child needs no
/// name lookups of its own, because the proxy resolves the CONNECT target,
/// and the invariant is that the proxy is the only egress. So the socket stays
/// closed there.
///
/// Once outbound is filtered, listening is too: a granted port is a port to
/// reach, not a port to serve on, and `--allow-bind` names the ones to serve.
pub(super) fn network_rules(allowed_net: &[String], proxy: bool, bind_ports: &[u16]) -> String {
    if allowed_net.is_empty() { return String::new(); }
    let mut rules = String::from("(deny network-outbound)\n(deny network-bind)\n(deny network-inbound)\n");
    if !proxy {
        rules.push_str(&format!("(allow network-outbound (literal \"{}\"))\n", RESOLVER_SOCKET));
    }
    for host in allowed_net {
        let Some((address, port)) = host.rsplit_once(':') else { continue };
        if port != "*" && !port.parse::<u16>().is_ok_and(|port| port > 0) { continue; }
        let address = if address == "127.0.0.1" || address == "localhost" { "localhost" } else { "*" };
        rules.push_str(&format!("(allow network-outbound (remote tcp \"{}:{}\"))\n", address, port));
    }
    for port in bind_ports {
        rules.push_str(&format!(
            "(allow network-bind (local tcp \"*:{port}\"))\n(allow network-inbound (local tcp \"*:{port}\"))\n"
        ));
    }
    rules
}

/// Credential-bearing Unix sockets are closed to connects in every mode, and
/// reopened only for the paths the caller names. Under `--allow-net` the
/// blanket outbound deny already closes them; here is where the open-network
/// default closes them too.
pub(super) fn socket_rules(deny_unix: &[String], allowed_unix: &[String]) -> String {
    let mut rules = String::new();
    // The patterns are regex source, not strings: a backslash in them is the
    // regex's own escape and must reach the kernel as written. A quote would
    // end the regex literal, so a pattern holding one is escaped as the
    // profile's strings are.
    for pattern in deny_unix {
        let pattern = pattern.replace('"', "\\\"");
        rules.push_str(&format!("(deny network-outbound (regex #\"{}\"))\n", pattern));
    }
    for path in allowed_unix {
        // The kernel matches against the path it resolved, so a socket reached
        // through a symlinked directory — `/var/run` is `/private/var/run` —
        // must be allowed by its resolved name, or the allow never fires and
        // the credential-socket deny above stands. Both spellings are emitted:
        // the resolved one for the match, the given one in case the socket
        // does not exist yet at profile-build time.
        rules.push_str(&format!("(allow network-outbound (literal \"{}\"))\n", sandbox_literal(path)));
        if let Ok(resolved) = std::fs::canonicalize(path) {
            let resolved = resolved.to_string_lossy();
            if resolved != *path {
                rules.push_str(&format!("(allow network-outbound (literal \"{}\"))\n", sandbox_literal(&resolved)));
            }
        }
    }
    rules
}
