//! The macOS sandbox profile.
//!
//! `sandbox-exec` takes one profile text, so the whole policy is written
//! here as three rule blocks: what may be written, what may not be read,
//! and which outbound ports are open.

/// Writable roots the macOS profile grants on every run. `/tmp` is reached
/// through `/private/tmp` there, so the profile has to name both spellings.
pub(crate) const PROFILE_WRITABLE: [&str; 3] = ["/tmp", "/private/tmp", "/dev"];

fn sandbox_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The whole profile for one request: everything `sandbox-exec` will apply.
pub(crate) fn build_sandbox_profile_rs(allowed_dirs: &[String], allowed_net: &[String]) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str(&write_rules(allowed_dirs));
    profile.push_str(&key_read_rules());
    profile.push_str(&network_rules(allowed_net));
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

/// Reads stay open, minus the two directories whose contents are keys. Landlock
/// cannot express this deny list, which is why the Linux path refuses instead.
fn key_read_rules() -> String {
    let Ok(home) = std::env::var("HOME") else { return String::new() };
    let home = sandbox_literal(&home);
    format!("(deny file-read-data (subpath \"{home}/.ssh\"))\n\
             (deny file-read-data (subpath \"{home}/.gnupg\"))\n")
}

/// The network is open like Docker's until `--allow-net` names a port, which
/// then closes everything else. Only the port is filtered, not the host.
fn network_rules(allowed_net: &[String]) -> String {
    if allowed_net.is_empty() { return String::new(); }
    let mut rules = String::from("(deny network-outbound)\n");
    for host in allowed_net {
        let Some((address, port)) = host.rsplit_once(':') else { continue };
        if port != "*" && !port.parse::<u16>().is_ok_and(|port| port > 0) { continue; }
        let address = if address == "127.0.0.1" || address == "localhost" { "localhost" } else { "*" };
        rules.push_str(&format!("(allow network-outbound (remote tcp \"{}:{}\"))\n", address, port));
    }
    rules
}

/// The profile a given set of mounts and ports produces, for `porta` to show.
pub fn wt_sandbox_profile(dirs_json: impl AsRef<str>, net_json: impl AsRef<str>) -> String {
    match (serde_json::from_str::<Vec<String>>(dirs_json.as_ref()), serde_json::from_str::<Vec<String>>(net_json.as_ref())) {
        (Ok(dirs), Ok(net)) => build_sandbox_profile_rs(&dirs, &net),
        _ => "(version 1)\n(deny default)\n".to_string(),
    }
}
