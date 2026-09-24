//! The macOS sandbox profile.
//!
//! `sandbox-exec` takes one profile text, so the whole policy is written here
//! as rule blocks: what may be written, what may be read, which outbound
//! channels are open, and the host facilities a confined command has no
//! business with. Where two rules match one operation the later one wins, so
//! every block below is ordered as "grant, then the denies that grant must not
//! reopen".

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

/// Mach services that give a process the login Keychain. Denying the files
/// alone is not enough: `security(1)` talks to these daemons, which read the
/// database on the caller's behalf.
const KEYCHAIN_SERVICES: [&str; 4] = [
    "com.apple.securityd.xpc",
    "com.apple.secd",
    "com.apple.security.agent",
    "com.apple.SecurityServer",
];

/// Mach services through which `open(1)` and Launch Services start a handler
/// *outside* this sandbox, with the caller's full privileges.
const LAUNCH_SERVICES: [&str; 3] =
    ["com.apple.lsd.mapdb", "com.apple.lsd.modifydb", "com.apple.quarantine-resolver"];

/// Daemons that mount volumes, join network shares, or drive other
/// applications by Apple Events. Nothing a confined command should reach.
const HOST_CONTROL_SERVICES: [&str; 4] = [
    "com.apple.DiskArbitration.diskarbitrationd",
    "com.apple.NetAuthAgent",
    "com.apple.NetAuthSysAgent",
    "com.apple.appleeventsd",
];

/// The socket every macOS resolver call goes through. `getaddrinfo` does not
/// send DNS itself; it asks mDNSResponder over this path, and that daemon —
/// outside the sandbox — does the lookup.
const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// Every root writable on every run. The caller's own `TMPDIR` under
/// `/private/var/folders` is deliberately not among them, and `TMPDIR` is not
/// passed into the sandbox either: that directory is where the caller's other
/// applications keep sockets and scratch state, and a command given `/tmp`
/// does not need it. Widening it here would have opened every file the
/// caller's tools left there to a strict run.
fn always_writable() -> Vec<String> {
    PROFILE_WRITABLE.iter().map(|dir| dir.to_string()).collect()
}

/// `value` as the inside of a profile string. Backslash and quote are
/// escaped, and so is every control character, as `\xHH`: a newline left raw
/// would carry the rule onto a second line, where the per-run tagging, which
/// works a line at a time, could land inside the string (found by
/// `scripts/fuzz.py policy`). The profile language reads `\xHH` back as the
/// byte it names, so the rule still names the directory on disk.
fn sandbox_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            ch if ch.is_control() && (ch as u32) < 0x80 => escaped.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

/// One request's policy inputs, as the profile builder reads them.
pub(crate) struct ProfileRequest<'a> {
    pub allowed_dirs: &'a [String],
    pub allowed_net: &'a [String],
    pub read_policy: &'a str,
    /// Whether the loopback endpoint in `allowed_net` is the only egress.
    pub proxy: bool,
    /// `--no-net`: every network operation denied, the resolver included.
    pub no_network: bool,
    /// TCP ports the command may listen on. Empty means none when a network
    /// rule is in force, and everything when the network is open.
    pub bind_ports: &'a [u16],
    /// Unix socket paths reopened for connects after the credential-socket
    /// denies.
    pub allowed_unix: &'a [String],
    /// What this run closes beyond its grants: the preset and the caller's
    /// own additions, resolved (see `policy_preset`).
    pub closures: &'a crate::policy_preset::Closures,
    /// This run's tag. Every deny rule carries it as its log message, so the
    /// kernel's denial records for this run can be told from every other
    /// process's. Empty for a profile that is only being shown.
    pub tag: &'a str,
}

/// The whole profile for one request: everything `sandbox-exec` will apply.
pub(crate) fn build_sandbox_profile(request: &ProfileRequest) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str(&write_rules(request.allowed_dirs, &request.closures.protect));
    profile.push_str(&read_rules(request.allowed_dirs, request.read_policy));
    profile.push_str(&closed_read_rules(&request.closures.deny_read));
    profile.push_str(&if request.no_network {
        NO_NETWORK_RULES.to_string()
    } else {
        network_rules(request.allowed_net, request.proxy, request.bind_ports)
    });
    profile.push_str(&socket_rules(&request.closures.deny_unix, request.allowed_unix));
    profile.push_str(&host_rules());
    tagged(&profile, request.tag)
}

/// The tag every deny rule of one run carries, as it appears in the kernel's
/// denial log. Defined here, beside the rules that carry it, and read by the
/// denial reader.
pub(crate) fn message_tag(run_tag: &str) -> String {
    format!("porta:{run_tag}")
}

/// Every deny rule with this run's tag as its message. A rule is one line and
/// ends with its closing parenthesis, so the message goes just before it.
///
/// The profile is split on `\n` alone. `str::lines` would also take a `\r`
/// before it, and a path may hold one: a mount named `repo\r\nx` came out of
/// the old split as `repo\nx`, a different directory, in every rule for it
/// (found by `scripts/fuzz.py policy`).
fn tagged(profile: &str, tag: &str) -> String {
    if tag.is_empty() {
        return profile.to_string();
    }
    let message = format!(" (with message \"{}\"))\n", message_tag(tag));
    profile
        .split_inclusive('\n')
        .map(|chunk| chunk.strip_suffix('\n').unwrap_or(chunk))
        .map(|line| {
            if line.starts_with("(deny ") && line.ends_with(')') {
                format!("{}{}", &line[..line.len() - 1], message)
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

/// The older calling shape, kept for the profile viewer and its tests.
pub(crate) fn build_sandbox_profile_rs(
    allowed_dirs: &[String],
    allowed_net: &[String],
    read_policy: &str,
    proxy: bool,
) -> String {
    let home = std::env::var("HOME").ok();
    let closures = crate::policy_preset::resolve("default", &Default::default(), home.as_deref()).unwrap_or_default();
    build_sandbox_profile(&ProfileRequest {
        allowed_dirs,
        allowed_net,
        read_policy,
        proxy,
        no_network: false,
        bind_ports: &[],
        allowed_unix: &[],
        closures: &closures,
        tag: "",
    })
}

/// Writes are denied first and reopened only for the granted mounts, so an
/// empty mount list leaves nothing writable but the always-writable roots.
/// After the grants come the denies a grant must not reopen: the files at a
/// mount's root a host tool trusts, the existing repository's hooks and
/// config, and the mount root itself, which stays where the policy put it.
fn write_rules(allowed_dirs: &[String], protect: &[String]) -> String {
    let mut rules = String::from("(deny file-write*)\n");
    let writable: Vec<&str> =
        allowed_dirs.iter().filter(|dir| !dir.ends_with(":ro")).map(|dir| dir.as_str()).collect();
    for dir in &writable {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for always in always_writable() {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", sandbox_literal(&always)));
    }
    for dir in &writable {
        rules.push_str(&mount_protection_rules(dir, protect));
    }
    rules
}

/// What stays closed inside one writable mount. Emitted after the grant.
///
/// A protected path can be swapped as well as written — renamed aside, a
/// replacement created, renamed back — so every ancestor of a protected path
/// up to the mount root is pinned against unlink and rename, and the mount
/// root is pinned too. A `.git` that does not exist yet is not protected: it
/// is the operator's repository this guards, not one the command creates.
fn mount_protection_rules(dir: &str, protect: &[String]) -> String {
    let mut rules = String::new();
    let mut pinned: Vec<String> = vec![dir.to_string()];
    // `subpath` covers a file as well as a directory and all beneath it, so
    // one rule serves either, and one created later is covered too.
    for name in protect {
        rules.push_str(&format!("(deny file-write* (subpath \"{}\"))\n", sandbox_literal(&format!("{dir}/{name}"))));
        let mut parent = std::path::Path::new(name.as_str()).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            pinned.push(format!("{dir}/{}", path.display()));
            parent = path.parent();
        }
    }
    if let Some(git_dir) = repository_dir(dir) {
        rules.push_str(&format!("(deny file-write* (subpath \"{}\"))\n", sandbox_literal(&format!("{git_dir}/hooks"))));
        rules.push_str(&format!("(deny file-write* (literal \"{}\"))\n", sandbox_literal(&format!("{git_dir}/config"))));
        pinned.push(git_dir.clone());
        pinned.push(format!("{dir}/.git"));
    }
    for path in pinned {
        rules.push_str(&format!("(deny file-write-unlink (literal \"{}\"))\n", sandbox_literal(&path)));
    }
    rules
}

/// The repository directory a mount root belongs to, if it has one: `.git`
/// itself, or the directory a `.git` pointer file names (a worktree or a
/// submodule), resolved as the kernel will see it.
pub(crate) fn repository_dir(dir: &str) -> Option<String> {
    let dot_git = std::path::Path::new(dir).join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;
    let target = if metadata.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let relative = pointer.trim().strip_prefix("gitdir:")?.trim();
        std::path::Path::new(dir).join(relative)
    };
    Some(std::fs::canonicalize(target).ok()?.to_string_lossy().to_string())
}

fn read_rules(allowed_dirs: &[String], read_policy: &str) -> String {
    if read_policy == "strict" { confined_read_rules(allowed_dirs) } else { String::new() }
}

/// The paths the preset and the caller close to reads, in every mode and
/// after every grant, so a mount cannot reopen them. `file-read*` rather than
/// `file-read-data`: listing a key directory already says which hosts and
/// accounts exist.
fn closed_read_rules(deny_read: &[String]) -> String {
    let mut rules = String::new();
    for path in deny_read {
        rules.push_str(&format!("(deny file-read* (subpath \"{}\"))\n", sandbox_literal(path)));
    }
    // The system keychain holds trust roots and nothing of the caller's, and
    // TLS needs it; the rest of that directory is other users' keychains.
    rules.push_str("(deny file-read* (subpath \"/Library/Keychains\"))\n");
    rules.push_str("(allow file-read* (literal \"/Library/Keychains/System.keychain\"))\n");
    rules
}

/// Reads are denied first and reopened for the granted mounts, the roots this
/// profile always makes writable, and the platform's own directories. Anything
/// the command needs beyond those — a language runtime's package directory,
/// say — is a mount the caller grants, not a hole this list leaves open.
fn confined_read_rules(allowed_dirs: &[String]) -> String {
    let mut rules = String::from("(deny file-read*)\n");
    let always = always_writable();
    let granted = allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir));
    for dir in granted.chain(always.iter().map(String::as_str)).chain(PROFILE_READABLE) {
        rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for path in PROFILE_READABLE_LITERALS {
        rules.push_str(&format!("(allow file-read* (literal \"{}\"))\n", path));
    }
    // A tool resolving its own path walks the ancestors of every mount with
    // stat(2); metadata on those, and only metadata, stays readable.
    for dir in allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir)) {
        let mut ancestor = std::path::Path::new(dir).parent();
        while let Some(path) = ancestor {
            if path.as_os_str().is_empty() || path == std::path::Path::new("/") { break; }
            rules.push_str(&format!(
                "(allow file-read-metadata (literal \"{}\"))\n",
                sandbox_literal(&path.to_string_lossy())
            ));
            ancestor = path.parent();
        }
    }
    rules
}

/// Everything a strict read policy leaves readable: nothing else on this host
/// can be opened, including the command porta is being asked to start. The
/// single-path literals are left out — a command is never one of them.
pub(crate) fn readable_roots(allowed_dirs: &[String]) -> Vec<String> {
    allowed_dirs
        .iter()
        .map(|dir| dir.strip_suffix(":ro").unwrap_or(dir).to_string())
        .chain(always_writable())
        .chain(PROFILE_READABLE.iter().map(|dir| dir.to_string()))
        .collect()
}

/// `--no-net`: no outbound, no listening, no inbound, and no resolver. The
/// Unix sockets `--allow-unix` names are reopened after this, as in every mode.
const NO_NETWORK_RULES: &str = "(deny network-outbound)\n(deny network-bind)\n(deny network-inbound)\n";

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
fn network_rules(allowed_net: &[String], proxy: bool, bind_ports: &[u16]) -> String {
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
fn socket_rules(deny_unix: &[String], allowed_unix: &[String]) -> String {
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

/// Host facilities a confined command has no reason to reach, closed in every
/// mode.
///
/// - Another process's arguments and environment: `sysctl kern.procargs2` and
///   `proc_pidinfo` hand them over for every process of the same user, and
///   credentials passed as flags live there. Both are re-allowed for the
///   sandbox's own processes.
/// - The login Keychain, through the daemons that read it on a caller's behalf.
/// - Launch Services: `open(1)` would start a handler outside the sandbox.
/// - Mounting, network shares, Apple Events, raw disks and packet capture.
/// - Two `fcntl` commands that change a file through a read-only descriptor,
///   and the anonymous XPC services that no command-line tool uses.
fn host_rules() -> String {
    let mut rules = String::from(
        "(deny sysctl-read (sysctl-name-regex #\"procargs\"))\n\
         (deny process-info-pidinfo)\n\
         (allow process-info-pidinfo (target same-sandbox))\n\
         (deny lsopen)\n\
         (deny file-mount)\n\
         (deny file-unmount)\n\
         (deny file-read* file-write* (regex #\"^/dev/r?disk\") (regex #\"^/dev/bpf\"))\n\
         (deny system-fcntl (fcntl-command 80 110))\n\
         (deny mach-lookup (xpc-service-name-prefix \"\"))\n",
    );
    for service in KEYCHAIN_SERVICES.iter().chain(LAUNCH_SERVICES.iter()).chain(HOST_CONTROL_SERVICES.iter()) {
        rules.push_str(&format!("(deny mach-lookup (global-name \"{}\"))\n", service));
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
