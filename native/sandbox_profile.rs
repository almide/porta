//! The macOS sandbox profile.
//!
//! `sandbox-exec` takes one profile text, so the whole policy is written
//! here as three rule blocks: what may be written, what may be read, and
//! which outbound ports are open.

/// Writable roots the macOS profile grants on every run. `/tmp` is reached
/// through `/private/tmp` there, so the profile has to name both spellings.
pub(crate) const PROFILE_WRITABLE: [&str; 3] = ["/tmp", "/private/tmp", "/dev"];

/// The platform's own directories, readable under a strict read policy. A
/// command cannot start without its interpreter, the shared library cache and
/// the system configuration it consults, so confining reads to the granted
/// mounts alone would only mean nothing runs. A caller's home directory is
/// deliberately absent: that is what this policy exists to close.
///
/// The set was read off the kernel's own denial records rather than guessed,
/// because the profile's `(trace ...)` facility is itself denied. See
/// `docs/roadmap/done/05-strict-read-policy.md`.
/// `/private/var/select` holds one symlink naming the shell `/bin/sh` should
/// behave as, which `/bin/sh` reads at startup; it is named here rather than
/// all of `/private/var`, which is where the system's own state lives.
const PROFILE_READABLE: [&str; 6] =
    ["/usr", "/System", "/bin", "/sbin", "/private/etc", "/private/var/select"];

/// Paths granted as themselves rather than as subtrees. The loader reads the
/// root directory entry before anything else, and `/tmp`, `/etc` and `/var`
/// are symlinks: granting the link exposes nothing, because whether its target
/// is readable is decided above.
const PROFILE_READABLE_LITERALS: [&str; 4] = ["/", "/tmp", "/etc", "/var"];

fn sandbox_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The whole profile for one request: everything `sandbox-exec` will apply.
pub(crate) fn build_sandbox_profile_rs(
    allowed_dirs: &[String],
    allowed_net: &[String],
    read_policy: &str,
    proxy: bool,
) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str(&write_rules(allowed_dirs));
    profile.push_str(&read_rules(allowed_dirs, read_policy));
    profile.push_str(&network_rules(allowed_net, proxy));
    profile
}

/// Writes are denied first and reopened only for the granted mounts, so an
/// empty mount list leaves nothing writable but the always-writable roots.
fn write_rules(allowed_dirs: &[String]) -> String {
    let mut rules = String::from("(deny file-write*)\n");
    for dir in allowed_dirs.iter().filter(|dir| !dir.ends_with(":ro")) {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for always in PROFILE_WRITABLE {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", always));
    }
    rules
}

fn read_rules(allowed_dirs: &[String], read_policy: &str) -> String {
    if read_policy == "strict" { confined_read_rules(allowed_dirs) } else { key_read_rules() }
}

/// Reads stay open, minus the two directories whose contents are keys. This is
/// the default because it is what a caller replacing `docker run` expects.
fn key_read_rules() -> String {
    let Ok(home) = std::env::var("HOME") else { return String::new() };
    let home = sandbox_literal(&home);
    format!("(deny file-read-data (subpath \"{home}/.ssh\"))\n\
             (deny file-read-data (subpath \"{home}/.gnupg\"))\n")
}

/// Reads are denied first and reopened for the granted mounts, the roots this
/// profile always makes writable, and the platform's own directories. Anything
/// the command needs beyond those — a language runtime's package directory,
/// say — is a mount the caller grants, not a hole this list leaves open.
fn confined_read_rules(allowed_dirs: &[String]) -> String {
    let mut rules = String::from("(deny file-read*)\n");
    let granted = allowed_dirs.iter().map(|dir| dir.trim_end_matches(":ro"));
    for dir in granted.chain(PROFILE_WRITABLE).chain(PROFILE_READABLE) {
        rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for path in PROFILE_READABLE_LITERALS {
        rules.push_str(&format!("(allow file-read* (literal \"{}\"))\n", path));
    }
    rules
}

/// Everything a strict read policy leaves readable: nothing else on this host
/// can be opened, including the command porta is being asked to start. The
/// single-path literals are left out — a command is never one of them.
pub(crate) fn readable_roots(allowed_dirs: &[String]) -> Vec<String> {
    allowed_dirs
        .iter()
        .map(|dir| dir.trim_end_matches(":ro"))
        .chain(PROFILE_WRITABLE)
        .chain(PROFILE_READABLE)
        .map(|dir| dir.to_string())
        .collect()
}

/// The socket every macOS resolver call goes through. `getaddrinfo` does not
/// send DNS itself; it asks mDNSResponder over this path, and that daemon —
/// outside the sandbox — does the lookup.
const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

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
fn network_rules(allowed_net: &[String], proxy: bool) -> String {
    if allowed_net.is_empty() { return String::new(); }
    let mut rules = String::from("(deny network-outbound)\n");
    if !proxy {
        rules.push_str(&format!("(allow network-outbound (literal \"{}\"))\n", RESOLVER_SOCKET));
    }
    for host in allowed_net {
        let Some((address, port)) = host.rsplit_once(':') else { continue };
        if port != "*" && !port.parse::<u16>().is_ok_and(|port| port > 0) { continue; }
        let address = if address == "127.0.0.1" || address == "localhost" { "localhost" } else { "*" };
        rules.push_str(&format!("(allow network-outbound (remote tcp \"{}:{}\"))\n", address, port));
    }
    rules
}

/// The profile a given set of mounts, ports and read policy produces, for
/// `porta` to show.
pub fn wt_sandbox_profile(
    dirs_json: impl AsRef<str>,
    net_json: impl AsRef<str>,
    read_policy: impl AsRef<str>,
) -> String {
    let dirs = serde_json::from_str::<Vec<String>>(dirs_json.as_ref());
    let net = serde_json::from_str::<Vec<String>>(net_json.as_ref());
    match (dirs, net) {
        (Ok(dirs), Ok(net)) => build_sandbox_profile_rs(&dirs, &net, read_policy.as_ref(), false),
        _ => "(version 1)\n(deny default)\n".to_string(),
    }
}
