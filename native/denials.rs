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

pub use crate::denial_advice::*;
use std::collections::BTreeSet;

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
    use crate::policy_preset::Closures;

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
        let closures = Closures::default();
        assert_eq!(advice(&denials[0], &closures), "-v /Users/x");
        assert_eq!(advice(&denials[1], &closures), "--allow-net '*:443'");
    }

    #[test]
    fn never_granted_things_say_so() {
        let closures = crate::policy_preset::resolve("default", &Closures::default(), Some("/Users/x")).unwrap();
        let keys = Denial { operation: "file-read-data".into(), target: "/Users/x/.ssh/id_ed25519".into() };
        assert!(advice(&keys, &closures).contains("closed by the preset"));
        let hook = Denial { operation: "file-write-create".into(), target: "/w/repo/.git/hooks/pre-commit".into() };
        assert!(advice(&hook, &closures).contains("protected inside the mount"));
        let agent = Denial { operation: "network-outbound".into(), target: "/private/tmp/com.apple.launchd.x/Listeners".into() };
        assert_eq!(advice(&agent, &closures), "--allow-unix /private/tmp/com.apple.launchd.x/Listeners");
        let bind = Denial { operation: "network-bind".into(), target: "local:*:8080".into() };
        assert_eq!(advice(&bind, &closures), "--allow-bind 8080");
    }
}
