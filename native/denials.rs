//! What the sandbox refused during a run, and what would have allowed it.
//!
//! A denied command fails with the tool's own message — `Operation not
//! permitted` — which looks exactly like a broken tool. The kernel knows
//! better: every Seatbelt denial is written to the unified log with the
//! operation and the path or address, and every deny rule porta emits carries
//! a per-run tag in its message, so the entries that belong to this run can
//! be picked out of everyone else's. After the child exits, porta reads them
//! back and says, for each, which flag would have granted it — or that
//! nothing would, because it is one of the things porta never grants.
#![cfg(target_os = "macos")]

use std::collections::BTreeSet;

/// One refusal, as the kernel recorded it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Denial {
    /// The Seatbelt operation: `file-write-create`, `network-outbound`, …
    pub operation: String,
    /// What it was tried on: a path, `remote:*:443`, a mach service name, or
    /// nothing for a bare network deny.
    pub target: String,
}

use crate::sandbox_profile::message_tag;

/// Denials the kernel logged for the run carrying `run_tag` since `since`.
/// Reads the unified log through `log show`, the only interface it has; the
/// query takes under a second, and a run that produced no denial pays it
/// only when it failed.
///
/// The log is written asynchronously, so a run that failed at once can exit
/// before its own denial is queryable. An empty answer is asked again, twice,
/// a little later; a run with nothing to report pays about a second more.
pub fn collect(run_tag: &str, since: &str) -> Vec<Denial> {
    let tag = message_tag(run_tag);
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        let output = std::process::Command::new("/usr/bin/log")
            .args(["show", "--start", since, "--style", "compact", "--predicate", "senderImagePath CONTAINS \"Sandbox\""])
            .output();
        let Ok(output) = output else { return Vec::new() };
        let denials = parse(&String::from_utf8_lossy(&output.stdout), &tag);
        if !denials.is_empty() {
            return denials;
        }
    }
    Vec::new()
}

/// Picks this run's denials out of the log text. A denial line reads
/// `… Sandbox: name(pid) deny(1) <operation> <target>` and the rule's message
/// follows on the next line; only entries whose message is this run's tag
/// count.
pub fn parse(log: &str, tag: &str) -> Vec<Denial> {
    let lines: Vec<&str> = log.lines().collect();
    let mut found = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(rest) = line.split_once(" deny(1) ").map(|(_, rest)| rest.trim()) else { continue };
        if lines.get(index + 1).map(|next| next.trim()) != Some(tag) {
            continue;
        }
        let (operation, target) = match rest.split_once(' ') {
            Some((operation, target)) => (operation.to_string(), target.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        found.insert(Denial { operation, target });
    }
    // A client refused a connection is often refused twice: once with the
    // address it asked for and once, from the same call, with none. The bare
    // record adds nothing when an addressed one is there.
    let addressed = found.iter().any(|denial| denial.operation == "network-outbound" && !denial.target.is_empty());
    found
        .into_iter()
        .filter(|denial| !(addressed && denial.operation == "network-outbound" && denial.target.is_empty()))
        .collect()
}

/// Paths under the home porta closes in every mode, mirrored from the
/// profile so the advice matches the rule.
const NEVER_GRANTED_UNDER_HOME: [&str; 10] = [
    ".ssh", ".gnupg", ".aws", ".config/gcloud", ".docker", ".kube", "Library/Keychains", "Library/Cookies",
    "Library/Application Support/Google/Chrome", "Library/Application Support/Firefox",
];

/// Names inside a writable mount that porta never lets a command write.
const PROTECTED_NAMES: [&str; 14] = [
    "/.git/hooks", "/.git/config", "/.bashrc", "/.bash_profile", "/.zshrc", "/.zprofile", "/.profile",
    "/.gitconfig", "/.mcp.json", "/.npmrc", "/porta.toml", "/.porta.toml", "/.claude/commands", "/.claude/agents",
];

/// The flag that would have allowed one denial, or why none would. Each
/// operation class has its own helper; this only routes to them.
pub fn advice(denial: &Denial) -> String {
    let operation = denial.operation.as_str();
    if operation.starts_with("file-write") || operation.starts_with("file-read") {
        return file_advice(operation, &denial.target);
    }
    if operation == "network-outbound" {
        return outbound_advice(&denial.target);
    }
    if operation == "network-bind" || operation == "network-inbound" {
        return match denial.target.rsplit_once(':') {
            Some((_, port)) => format!("--allow-bind {port}"),
            None => "--allow-bind <port>".to_string(),
        };
    }
    match operation {
        "mach-lookup" => "a host service porta closes (Keychain, Launch Services, disks); no flag opens it".to_string(),
        "lsopen" => "open(1) would start a program outside the sandbox; porta never grants it".to_string(),
        op if op.starts_with("sysctl") || op.starts_with("process-info") => "another process's details; porta never grants them".to_string(),
        _ => "not something a flag grants".to_string(),
    }
}

/// The `-v` grant for a denied read or write, or why none is offered.
fn file_advice(operation: &str, target: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if NEVER_GRANTED_UNDER_HOME.iter().any(|dir| target.starts_with(&format!("{home}/{dir}"))) {
        return "a credential store; porta never grants it".to_string();
    }
    if PROTECTED_NAMES.iter().any(|name| target.contains(name)) {
        return "protected inside the mount; porta never grants it".to_string();
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
pub fn footer(denials: &[Denial], rerun: &str) -> String {
    if denials.is_empty() {
        return String::new();
    }
    let noun = if denials.len() == 1 { "time" } else { "times" };
    let mut text = format!("[porta] the sandbox refused this run {} {noun}; what each would have needed:\n", denials.len());
    let mut grants: Vec<String> = Vec::new();
    for denial in denials {
        let what = if denial.target.is_empty() { denial.operation.clone() } else { format!("{} {}", denial.operation, denial.target) };
        let advice = advice(denial);
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

/// The instant a run started, as `log show --start` wants it: local time,
/// to the second.
pub fn now_for_log() -> String {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now.tv_sec, &mut local) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.tm_year + 1900,
        local.tm_mon + 1,
        local.tm_mday,
        local.tm_hour,
        local.tm_min,
        local.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_only_this_runs_denials() {
        let log = "\
2026-09-21 13:40:47 E  kernel[0:1] (Sandbox) Sandbox: spotlightknowledged(1485) deny(1) mach-lookup com.apple.PowerManagement.control
2026-09-21 13:40:48 E  kernel[0:1] (Sandbox) Sandbox: bash(13486) deny(1) file-write-create /Users/x/out.txt
porta:abc
2026-09-21 13:40:48 E  kernel[0:1] (Sandbox) Sandbox: curl(13590) deny(1) network-outbound remote:*:443
porta:abc
2026-09-21 13:40:48 E  kernel[0:1] (Sandbox) Sandbox: curl(13591) deny(1) network-outbound remote:*:443
porta:other
";
        let denials = parse(log, "porta:abc");
        assert_eq!(denials.len(), 2);
        assert_eq!(denials[0].operation, "file-write-create");
        assert_eq!(advice(&denials[0]), "-v /Users/x");
        assert_eq!(advice(&denials[1]), "--allow-net '*:443'");
    }

    #[test]
    fn never_granted_things_say_so() {
        std::env::set_var("HOME", "/Users/x");
        let keys = Denial { operation: "file-read-data".into(), target: "/Users/x/.ssh/id_ed25519".into() };
        assert!(advice(&keys).contains("never grants"));
        let hook = Denial { operation: "file-write-create".into(), target: "/w/repo/.git/hooks/pre-commit".into() };
        assert!(advice(&hook).contains("protected inside the mount"));
        let agent = Denial { operation: "network-outbound".into(), target: "/private/tmp/com.apple.launchd.x/Listeners".into() };
        assert_eq!(advice(&agent), "--allow-unix /private/tmp/com.apple.launchd.x/Listeners");
        let bind = Denial { operation: "network-bind".into(), target: "local:*:8080".into() };
        assert_eq!(advice(&bind), "--allow-bind 8080");
    }
}
