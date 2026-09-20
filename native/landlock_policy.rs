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
const SYSTEM_READABLE: [&str; 7] = ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc"];

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
    allowed_dirs.iter().map(|dir| dir.trim_end_matches(":ro").to_string()).collect()
}

/// The Landlock policy a request asks for, or why this kernel cannot apply it.
pub(crate) fn ruleset(
    allowed_dirs: &[String],
    allowed_net: &[String],
    read_policy: &str,
) -> Result<crate::landlock::Ruleset, String> {
    let strict = read_policy == "strict";
    let policy = crate::landlock::Policy {
        writable_dirs: writable_dirs(allowed_dirs),
        readable_dirs: if strict { readable_dirs(allowed_dirs) } else { Vec::new() },
        system_dirs: SYSTEM_READABLE.iter().map(|dir| dir.to_string()).collect(),
        restrict_reads: strict,
        tcp_ports: requested_tcp_ports(allowed_net)?,
        restrict_network: !allowed_net.is_empty(),
    };
    crate::landlock::prepare(&policy)
}
