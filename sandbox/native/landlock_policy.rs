//! The Linux side of one sandboxed request, as `sandbox_profile` is the macOS
//! side: the mounts, ports and read policy a caller asked for, turned into the
//! Landlock ruleset that will be applied — or into the reason this kernel
//! cannot apply it.
#![cfg(target_os = "linux")]

/// The same roots as [`crate::sandbox_profile::PROFILE_WRITABLE`], as Linux
/// spells them: there is no `/private/tmp` to name.
const ALWAYS_WRITABLE: [&str; 2] = ["/tmp", "/dev"];

/// The platform's own directories, readable under a strict read policy. A
/// dynamically linked command cannot start without its interpreter, its
/// libraries and the loader cache, so confining reads to the granted mounts
/// alone would only mean nothing runs. A caller's home directory is
/// deliberately absent: that is what this policy exists to close.
///
/// `/proc` is deliberately absent too, and that is a security decision rather
/// than a loader one. Granting it hands a confined command the command line of
/// every other process the same user is running — credentials included — plus
/// the host's connection table and mount layout. Its own entry cannot be
/// granted either, not usefully: `/proc/self` names whoever opens it, so a
/// rule the exec'd command adds covers that command and none of the tools it
/// starts (the note at the end of `landlock.rs` records the measurement). A
/// command that needs `/proc` takes it as a mount the caller grants.
///
/// `/etc` is absent as a directory. It holds what a command needs to start —
/// the loader cache, the resolver configuration, the trust store — beside
/// `shadow`, `gshadow`, `sudoers` and the host's SSH keys, and Landlock cannot
/// grant a directory minus some of its files. So the needed files are granted
/// one by one, in [`SYSTEM_FILES`] and [`SYSTEM_ETC_DIRS`], and the rest of
/// `/etc` is closed.
const SYSTEM_READABLE: [&str; 5] = ["/usr", "/lib", "/lib64", "/bin", "/sbin"];

/// Subdirectories of `/etc` a command reads to start: the trust stores, the
/// loader's configuration fragments, the alternatives links Debian routes
/// `/usr/bin` names through, terminal descriptions and profile fragments.
const SYSTEM_ETC_DIRS: [&str; 9] = [
    "/etc/ssl",
    "/etc/pki",
    "/etc/ca-certificates",
    "/etc/crypto-policies",
    "/etc/ld.so.conf.d",
    "/etc/alternatives",
    "/etc/profile.d",
    "/etc/terminfo",
    "/etc/default",
];

/// Single files under `/etc` a command reads to start, or to resolve a name,
/// a user, a zone or a service. Absent ones are skipped. Nothing here is a
/// secret to the user running porta, and the files that are — `shadow`, the
/// host keys, `sudoers` — are not here.
const SYSTEM_FILES: [&str; 28] = [
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.preload",
    "/etc/localtime",
    "/etc/timezone",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/host.conf",
    "/etc/nsswitch.conf",
    "/etc/gai.conf",
    "/etc/passwd",
    "/etc/group",
    "/etc/services",
    "/etc/protocols",
    "/etc/os-release",
    "/etc/hostname",
    "/etc/machine-id",
    "/etc/shells",
    "/etc/environment",
    "/etc/profile",
    "/etc/bash.bashrc",
    "/etc/inputrc",
    "/etc/mime.types",
    "/etc/gitconfig",
    "/etc/ca-certificates.conf",
    "/etc/locale.alias",
    "/etc/locale.gen",
    "/etc/login.defs",
];

/// TCP ports from `--allow-net` entries, or the entry that cannot be expressed.
fn requested_tcp_ports(allowed_net: &[String]) -> Result<Vec<u16>, String> {
    let mut ports = Vec::new();
    for entry in allowed_net {
        let port = match entry.rsplit_once(':') {
            Some((_, port)) => port,
            None => return Err(format!("--allow-net entry has no port: {}", entry)),
        };
        // Landlock allows named ports only; "any port" cannot be expressed, and
        // silently treating it as "all open" would hide an unenforced rule.
        match port.parse::<u16>() {
            Ok(parsed) if parsed > 0 => ports.push(parsed),
            _ => return Err(format!(
                "Landlock cannot express the port in --allow-net {}; name a numeric TCP port",
                entry
            )),
        }
    }
    Ok(ports)
}

fn writable_dirs(allowed_dirs: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = allowed_dirs
        .iter()
        .filter(|dir| !dir.ends_with(":ro"))
        .map(|dir| dir.to_string())
        .collect();
    dirs.extend(ALWAYS_WRITABLE.iter().map(|dir| dir.to_string()));
    dirs
}

/// Every mount the caller was granted, read-only ones included. Under a strict
/// read policy these and the system directories are the only readable roots.
fn readable_dirs(allowed_dirs: &[String]) -> Vec<String> {
    allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir).to_string()).collect()
}

/// Everything a strict read policy leaves readable that a command could live
/// in: nothing else on this host can be opened, including the command porta
/// is being asked to start. The `/etc` files are left out — a command is
/// never one of them.
pub(crate) fn readable_roots(allowed_dirs: &[String]) -> Vec<String> {
    let mut roots = readable_dirs(allowed_dirs);
    roots.extend(SYSTEM_READABLE.iter().chain(ALWAYS_WRITABLE.iter()).map(|dir| dir.to_string()));
    roots
}

/// A request's TCP rules: the ports it may reach and serve on, and whether
/// TCP is closed even with no port named (`--no-net` without a namespace).
pub(crate) struct Network<'a> {
    pub(crate) connect: &'a [String],
    pub(crate) bind: &'a [u16],
    pub(crate) closed: bool,
}

/// The closed paths that exist on this host, as given (already resolved).
fn existing(closed: &[String]) -> Vec<String> {
    closed.iter().filter(|path| std::path::Path::new(path).exists()).cloned().collect()
}

/// A closed path inside a mount, or inside what strict reads leave open, is a
/// hole Landlock cannot cut: a grant covers everything beneath it. Without a
/// mount namespace to cover the path instead, the run is refused.
fn refuse_closed_inside_grants(allowed_dirs: &[String], closed: &[String], strict: bool) -> Result<(), String> {
    // /tmp and /dev are granted to every run, so a closed path under them is
    // inside a grant too.
    let mut grants: Vec<String> = allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir).to_string()).collect();
    grants.extend(ALWAYS_WRITABLE.iter().map(|dir| dir.to_string()));
    if strict {
        grants.extend(SYSTEM_READABLE.iter().map(|dir| dir.to_string()));
    }
    for path in existing(closed) {
        if let Some(grant) = grants.iter().find(|grant| path == **grant || path.starts_with(&format!("{grant}/"))) {
            return Err(format!(
                "{path} is closed to reads (the preset or --deny-read), but it lies inside {grant}, which this run grants; \
                 Landlock cannot close part of a grant and this host gives no mount namespace to cover it. \
                 Grant less, or drop the closure with --preset none or a preset without it"
            ));
        }
    }
    Ok(())
}

/// The Landlock policy a request asks for, or why this kernel cannot apply it.
pub(crate) fn ruleset(allowed_dirs: &[String], network: Network, read_policy: &str, closed: &[String]) -> Result<crate::landlock::Ruleset, String> {
    let strict = read_policy == "strict";
    let policy = crate::landlock::Policy {
        writable_dirs: writable_dirs(allowed_dirs),
        readable_dirs: if strict { readable_dirs(allowed_dirs) } else { Vec::new() },
        system_dirs: SYSTEM_READABLE.iter().chain(SYSTEM_ETC_DIRS.iter()).map(|dir| dir.to_string()).collect(),
        system_files: SYSTEM_FILES.iter().map(|file| file.to_string()).collect(),
        restrict_reads: strict,
        tcp_ports: requested_tcp_ports(network.connect)?,
        bind_ports: network.bind.to_vec(),
        restrict_network: network.closed || !network.connect.is_empty(),
        open_reads_except: if strict { Vec::new() } else { existing(closed) },
    };
    refuse_closed_inside_grants(allowed_dirs, closed, strict)?;
    crate::landlock::prepare(&policy)
}
