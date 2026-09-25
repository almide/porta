//! What one refusal of the sandbox needed: the flag that would have allowed
//! it, or why none would, and the footer that lists them after a run. The
//! refusals come from the unified log on macOS (`denials`) and from a traced
//! run on Linux (`--why`); the advice is the same.

use crate::policy_preset::Closures;

/// One refusal, as the kernel recorded it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Denial {
    /// The Seatbelt operation: `file-write-create`, `network-outbound`, …
    pub operation: String,
    /// What it was tried on: a path, `remote:*:443`, a mach service name, or
    /// nothing for a bare network deny.
    pub target: String,
}

/// A refused connection or listen: which port or host flag would have let it.
fn network_advice(operation: &str, target: &str) -> String {
    match (operation, target.rsplit_once(':')) {
        ("network-outbound", _) => outbound_advice(target),
        ("network-bind" | "network-inbound", Some((_, port))) => format!("--allow-bind {port}"),
        ("network-bind" | "network-inbound", None) => "--allow-bind <port>".to_string(),
        ("network-socket", _) => "only a TCP socket leaves under --allow-net or a proxy; no flag opens another kind".to_string(),
        _ => "not something a flag grants".to_string(),
    }
}

/// The flag that would have allowed one denial, or why none would. Each
/// operation class has its own helper; this only routes to them.
pub fn advice(denial: &Denial, closures: &Closures) -> String {
    let operation = denial.operation.as_str();
    if operation.starts_with("file-write") || operation.starts_with("file-read") {
        return file_advice(operation, &denial.target, closures);
    }
    if operation.starts_with("network-") {
        return network_advice(operation, &denial.target);
    }
    match operation {
        "mach-lookup" => "a host service porta closes (Keychain, Launch Services, disks); no flag opens it".to_string(),
        "lsopen" => "open(1) would start a program outside the sandbox; porta never grants it".to_string(),
        op if op.starts_with("sysctl") || op.starts_with("process-info") => "another process's details; porta never grants them".to_string(),
        _ => "not something a flag grants".to_string(),
    }
}

/// The `-v` grant for a denied read or write, or why none is offered.
fn file_advice(operation: &str, target: &str, closures: &Closures) -> String {
    let under = |path: &str| target == path || target.starts_with(&format!("{path}/"));
    if operation.starts_with("file-read") && closures.deny_read.iter().any(|path| under(path)) {
        return "closed by the preset or --deny-read; no mount reopens it (--preset none, or a preset without it, does)".to_string();
    }
    if closures.repository.iter().any(|name| target.contains(&format!("/.git/{name}"))) || closures.protect.iter().any(|name| if name.starts_with('/') { under(name) } else { target.contains(&format!("/{name}")) }) {
        return "protected inside the mount by the preset or --protect; no mount reopens it".to_string();
    }
    let dir = grantable_directory(target);
    if operation.starts_with("file-write") { format!("-v {dir}") } else { format!("-v {dir}:ro") }
}

/// The network flag for a denied outbound connection.
fn outbound_advice(target: &str) -> String {
    match target.strip_prefix("remote:") {
        Some(endpoint) => match endpoint.rsplit_once(':') {
            Some((_, port)) => format!("--allow-net '*:{port}'"),
            None => format!("--allow-net '{endpoint}'"),
        },
        None if target.starts_with('/') => format!("--allow-unix {target}"),
        None => "the network is closed by --allow-net; name the port to reach".to_string(),
    }
}

/// The directory a `-v` grant would name for a denied path: the path itself
/// when it is a directory, otherwise the directory it is in.
fn grantable_directory(target: &str) -> String {
    let path = std::path::Path::new(target);
    if path.is_dir() {
        return target.to_string();
    }
    path.parent().map(|parent| parent.to_string_lossy().to_string()).filter(|parent| !parent.is_empty()).unwrap_or_else(|| "/".to_string())
}

/// The footer porta prints after a run that was denied something: each
/// refusal with the flag it needed, then the command line to run again with
/// every grantable one added. `rerun` is that command line without them.
pub fn footer(denials: &[Denial], rerun: &str, closures: &Closures) -> String {
    if denials.is_empty() {
        return String::new();
    }
    let noun = if denials.len() == 1 { "time" } else { "times" };
    let mut text = format!("[porta] the sandbox refused this run {} {noun}; what each would have needed:\n", denials.len());
    let mut grants: Vec<String> = Vec::new();
    for denial in denials {
        let what = if denial.target.is_empty() { denial.operation.clone() } else { format!("{} {}", denial.operation, denial.target) };
        let advice = advice(denial, closures);
        text.push_str(&format!("  {what}\n    → {advice}\n"));
        if advice.starts_with('-') && !grants.contains(&advice) {
            grants.push(advice);
        }
    }
    if !grants.is_empty() && !rerun.is_empty() {
        text.push_str(&format!("  to run again with those granted:\n    {}\n", with_grants(rerun, &grants)));
    }
    text
}

/// `rerun` with `grants` inserted before its `--`, or at its end.
fn with_grants(rerun: &str, grants: &[String]) -> String {
    let added = grants.join(" ");
    match rerun.split_once(" -- ") {
        Some((head, tail)) => format!("{head} {added} -- {tail}"),
        None => format!("{rerun} {added}"),
    }
}
