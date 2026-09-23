#![cfg(target_os = "linux")]
//! A resident-memory ceiling for a native run: cgroup v2 `memory.max`, the
//! only limit that means what people mean by "memory". An rlimit caps address
//! space, which modern runtimes reserve by the gigabyte, and `RLIMIT_RSS` is
//! not enforced.
//!
//! porta runs unprivileged and its own cgroup is usually a login scope full
//! of other processes, so it cannot create a memory-limited child cgroup by
//! hand: cgroup v2 lets a non-root cgroup hand a controller to its children
//! only when it holds no processes itself. What an unprivileged user does have
//! is the systemd user manager, which owns a delegated subtree and will place
//! a process into a new transient scope with `MemoryMax` on request. That is
//! what `systemd-run --user --scope` does for its own process; here the
//! request names the stopped child instead, over the same D-Bus call.
//!
//! No user manager, no ceiling: the run is refused with the reason, never run
//! with less. The scope is the manager's to remove once it is empty.

use std::path::PathBuf;
use std::process::Command;

fn runtime_dir() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
        // Our own uid, read from /proc/self rather than asked of libc.
        let uid = std::fs::metadata("/proc/self").map(|meta| meta.uid()).unwrap_or(0);
        PathBuf::from(format!("/run/user/{uid}"))
    })
}

fn busctl() -> Option<PathBuf> {
    ["/usr/bin/busctl", "/bin/busctl"].iter().map(PathBuf::from).find(|path| path.exists())
}

/// Why a memory ceiling cannot be applied on this host, if it cannot.
pub fn unavailable() -> Option<String> {
    if busctl().is_none() {
        return Some("a memory ceiling is a cgroup v2 limit set through the systemd user manager, and busctl is not installed".into());
    }
    let bus = runtime_dir().join("bus");
    if !bus.exists() {
        return Some(format!(
            "a memory ceiling is a cgroup v2 limit set through the systemd user manager, and its bus {} is not there; \
             log in through systemd-logind, or run `loginctl enable-linger $USER` once, so a user manager runs for you",
            bus.display()
        ));
    }
    None
}

/// Places the stopped child `pid` into a fresh transient scope capped at
/// `bytes` of resident memory with swap closed, then reads the kernel back to
/// confirm both happened. Any step short of that is a refusal.
fn confine(pid: libc::pid_t, tag: &str, bytes: u64) -> Result<(), String> {
    let unit = format!("porta-{tag}.scope");
    let runtime = runtime_dir();
    let output = Command::new(busctl().ok_or("busctl is not installed")?)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", runtime.join("bus").display()))
        .args(["--user", "call", "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager"])
        .args(["StartTransientUnit", "ssa(sv)a(sa(sv))", &unit, "fail", "3"])
        .args(["PIDs", "au", "1", &pid.to_string()])
        .args(["MemoryMax", "t", &bytes.to_string()])
        .args(["MemorySwapMax", "t", "0", "0"])
        .output()
        .map_err(|error| format!("cannot run busctl: {error}"))?;
    if !output.status.success() {
        return Err(format!("the systemd user manager refused the scope: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    // The manager answers with a job and moves the process a moment later;
    // wait for the kernel to show it there, bounded, then read the ceiling.
    let mut path = String::new();
    for _ in 0..50 {
        path = cgroup_path(pid)?;
        if path.ends_with(&unit) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !path.ends_with(&unit) {
        return Err(format!("the command was not moved into its scope within five seconds (it is in {path})"));
    }
    let max = std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.max")).map_err(|error| format!("cannot read memory.max: {error}"))?;
    if max.trim() != bytes.to_string() {
        return Err(format!("memory.max reads {} where {bytes} was asked for", max.trim()));
    }
    Ok(())
}

/// The cgroup v2 path `pid` is in right now, from `/proc/<pid>/cgroup`.
fn cgroup_path(pid: libc::pid_t) -> Result<String, String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).map_err(|error| format!("cannot read the command's cgroup: {error}"))?;
    cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim().to_string())
        .ok_or_else(|| "the command's cgroup is not a cgroup v2 path".to_string())
}

/// The whole handshake for a freshly spawned child that stops itself after
/// applying its policy: wait for the stop, place it under the ceiling, let it
/// run. A child that cannot be placed is ended with its group, so a run never
/// proceeds without the ceiling it asked for.
pub fn place(child: &mut std::process::Child, tag: &str, bytes: u64) -> Result<(), String> {
    let pid = child.id() as libc::pid_t;
    let placed = await_stop(pid).and_then(|_| confine(pid, tag, bytes));
    if let Err(reason) = placed {
        crate::ceilings::kill_group(child);
        return Err(format!("--max-memory-mb: {reason}"));
    }
    unsafe { libc::kill(pid, libc::SIGCONT) };
    Ok(())
}

/// The same handshake for a child in its own namespaces (`pid_namespace`),
/// run on a thread while `spawn` waits: the outermost child sends its pid and
/// waits at the gate before it forks the command, so placing it places
/// everything the run will start. The go-ahead is one byte; a gate closed
/// without it ends the child before the command exists.
pub fn place_at_gate(
    mut ready: std::io::PipeReader,
    mut go: std::io::PipeWriter,
    tag: String,
    bytes: u64,
) -> std::thread::JoinHandle<Result<(), String>> {
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut pid = [0u8; 4];
        ready.read_exact(&mut pid).map_err(|_| "--max-memory-mb: the command ended before its memory ceiling could be applied".to_string())?;
        confine(libc::pid_t::from_ne_bytes(pid), &tag, bytes).map_err(|reason| format!("--max-memory-mb: {reason}"))?;
        go.write_all(&[1]).map_err(|error| format!("--max-memory-mb: cannot let the command start: {error}"))
    })
}

/// Waits for the child to stop itself (it raises SIGSTOP after applying its
/// policy), so it can be moved before it runs a single instruction of the
/// command. A child that exited instead reports that.
fn await_stop(pid: libc::pid_t) -> Result<(), String> {
    let mut status: libc::c_int = 0;
    loop {
        let waited = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if waited == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(format!("waiting for the command to pause failed: {error}"));
        }
        if libc::WIFSTOPPED(status) {
            return Ok(());
        }
        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            return Err("the command ended before its memory ceiling could be applied".into());
        }
    }
}
