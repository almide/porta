//! Restricted execution of native commands.
//!
//! One parsed request drives three entry points — capture the output, replace
//! this process, or supervise a child — and each platform applies what it can
//! express, refusing the run when it cannot express a requested rule.

use crate::ceilings::{apply_ceilings, unsettable_ceiling, wait_within, Ceiling, MIB};
use crate::json_text::escape_json_text;
#[cfg(target_os = "linux")]
use crate::landlock_policy::readable_roots;
#[cfg(target_os = "macos")]
use crate::sandbox_profile::{build_sandbox_profile, readable_roots, ProfileRequest};

mod checks;
mod command;
use command::{with_ceilings, INHERITED_ENV};
use checks::{expand_home, missing_command, resolve_mount};
mod explain;
pub use explain::{wt_sandbox_explain, wt_sandbox_explain_json};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod why;
#[cfg(target_os = "linux")]
use linux::{exec_sandboxed_linux, replace_with_sandboxed, supervise_sandboxed, RESOLVER_OVER_TCP};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::{exec_sandboxed_macos, replace_with_sandboxed, supervise_sandboxed};

/// One sandboxed execution request, read once and shared by all three entry
/// points. It arrives as a single document, so a caller cannot transpose two
/// of the policy lists on the way in.
#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SandboxRequest {
    cmd: String,
    args: Vec<String>,
    #[serde(rename = "dirs")] allowed_dirs: Vec<String>,
    #[serde(rename = "net")] allowed_net: Vec<String>,
    #[serde(rename = "env")] env_vars: Vec<(String, String)>,
    cwd: String,
    /// "open" leaves reads unrestricted; "strict" confines them to the granted
    /// mounts and the platform's own directories. Anything else is refused.
    #[serde(default = "open_reads")] read_policy: String,
    /// Whether this run's only permitted egress is the loopback proxy named in
    /// `net`. It is not inferable from `net` — a caller may grant a loopback
    /// port for its own reasons — and it decides whether every non-TCP egress
    /// channel has to be closed as well.
    #[serde(default)] proxy: bool,
    /// Whether the caller has said, in so many words, that running as root is
    /// what they meant. Nothing infers it.
    #[serde(default)] allow_root: bool,
    /// TCP ports the command may listen on once a network rule is in force.
    #[serde(rename = "bind", default)] allowed_bind: Vec<String>,
    /// No network at all. On Linux the command gets a network namespace of its
    /// own holding only a loopback interface, where the host gives one;
    /// otherwise Landlock refuses every TCP port and seccomp every other
    /// family. A kernel that can do neither refuses the run.
    #[serde(rename = "no_net", default)] no_network: bool,
    /// The preset this run starts from: `default`, `none`, or a preset file.
    /// It and the three lists after it are what the run closes beyond its
    /// grants; `closures` is them resolved (see `policy_preset`).
    #[serde(default)] preset: String,
    #[serde(default)] deny_read: Vec<String>,
    #[serde(default)] protect: Vec<String>,
    #[serde(default)] deny_unix: Vec<String>,
    #[serde(skip)] closures: crate::policy_preset::Closures,
    /// Unix socket paths the command may connect to although they hold a
    /// credential agent. Empty by default: the SSH agent, gpg-agent and the
    /// container runtimes are closed unless named.
    #[serde(rename = "unix", default)] allowed_unix: Vec<String>,
    /// Seconds the command may run before porta kills it and everything it
    /// started. 0 means no limit. A native command is a process on the host,
    /// so nothing else bounds its wall-clock; an agent that hangs or loops
    /// runs forever without this.
    #[serde(default)] timeout: u64,
    /// Resource ceilings set with `setrlimit` before exec and inherited by
    /// everything the command starts. Each is per process, not per run: a tree
    /// of processes gets the budget once each, and `timeout` bounds the whole.
    /// 0 leaves one unset. CPU is in seconds; the file size is in MiB; the
    /// process count is the kernel's, which counts every process of this user.
    #[serde(default)] max_cpu: u64,
    #[serde(default)] max_procs: u64,
    #[serde(default)] max_file_size: u64,
    /// Resident memory, in MiB, for the command and everything it starts,
    /// together: a cgroup v2 ceiling set through the systemd user manager on
    /// Linux, with swap closed. 0 leaves it unset. The one per-run ceiling.
    #[serde(default)] max_memory_mb: u64,
    /// This run's tag, minted here rather than sent: the mark every deny rule
    /// carries so the kernel's denial records for this run can be found.
    #[serde(skip)] tag: String,
    /// Say afterwards what the sandbox refused, even when the run succeeded.
    /// On Linux this runs the command under `strace` (see `why`).
    #[serde(default)] why: bool,
    /// Copy the writable mounts first, so `porta rollback` can put them back.
    #[serde(default)] snapshot: bool,
    /// The document this request was read from, for the copy of porta that
    /// `--why` starts under `strace` to apply it to itself.
    #[serde(skip)] raw: String,
}

/// A tag for one run: the pid and the clock, which no two runs on one host
/// share. It is a label for log lines, not a secret.
fn run_tag() -> String {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    format!("{:x}{:x}", std::process::id(), now.tv_nsec)
}

/// The bind ports a request names, or the entry that is not a port.
fn bind_ports(entries: &[String]) -> Result<Vec<u16>, String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .ok_or_else(|| format!("--allow-bind takes a TCP port, not {entry}"))
        })
        .collect()
}

fn open_reads() -> String { "open".to_string() }

/// Read policies this build understands.
const READ_POLICIES: [&str; 2] = ["open", "strict"];

impl SandboxRequest {
    /// Reads one request document. A document that does not parse refuses the
    /// run: an empty request would grant nothing but would also say nothing.
    fn parse(request_json: &str) -> Result<Self, String> {
        let mut request: Self = serde_json::from_str(request_json)
            .map_err(|error| format!("invalid sandbox request: {error}"))?;
        request.tag = run_tag();
        request.raw = request_json.to_string();
        let own = crate::policy_preset::Closures {
            deny_read: request.deny_read.clone(),
            protect: request.protect.clone(),
            deny_unix: request.deny_unix.clone(),
        };
        request.closures = crate::policy_preset::resolve(&request.preset, &own, std::env::var("HOME").ok().as_deref())?;
        if !READ_POLICIES.contains(&request.read_policy.as_str()) {
            return Err(format!("unknown read policy: {}; use open or strict", request.read_policy));
        }
        if let Some(reason) = request.running_as_root() {
            return Err(reason);
        }
        request.cwd = expand_home(&request.cwd);
        request.allowed_dirs =
            request.allowed_dirs.iter().map(|dir| resolve_mount(dir)).collect::<Result<_, _>>()?;
        if let Some(reason) = missing_command(&request.cmd, &request.cwd) {
            return Err(reason);
        }
        bind_ports(&request.allowed_bind)?;
        if let Some(reason) = request.contradictory_network() {
            return Err(reason.into());
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(reason) = request.unreadable_command() {
            return Err(reason);
        }
        if let Some(reason) = unsettable_ceiling(&request.ceilings()) {
            return Err(reason);
        }
        if let Some(reason) = request.memory_ceiling_unavailable() {
            return Err(reason);
        }
        Ok(request)
    }
}

pub use crate::http_proxy::{wt_is_host_allowed, wt_proxy_start, wt_proxy_stop};

/// Execute a command inside an OS-level sandbox.
/// Returns JSON: {"exit_code":0,"stdout":"...","stderr":"..."} or {"error":"..."}
pub fn wt_exec_sandboxed(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => run_sandboxed(&request),
        Err(reason) => json_error(&reason),
    }
}

#[cfg(target_os = "macos")]
fn run_sandboxed(request: &SandboxRequest) -> String {
    exec_sandboxed_macos(request)
}

#[cfg(target_os = "linux")]
fn run_sandboxed(request: &SandboxRequest) -> String {
    exec_sandboxed_linux(request)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run_sandboxed(_request: &SandboxRequest) -> String {
    "{\"error\":\"sandboxed execution not supported on this platform\"}".to_string()
}

/// The single shape every sandboxed execution reports, whatever enforced it.
fn finish_sandboxed(result: std::io::Result<std::process::Output>) -> String {
    let output = match result {
        Ok(output) => output,
        Err(e) => return format!("{{\"error\":\"sandbox exec failed: {}\"}}", e),
    };
    let exit_code = output.status.code().unwrap_or(-1);
    format!(
        "{{\"exit_code\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}",
        exit_code,
        escape_json_text(&String::from_utf8_lossy(&output.stdout)),
        escape_json_text(&String::from_utf8_lossy(&output.stderr)),
    )
}

fn json_error(reason: &str) -> String {
    format!("{{\"error\":\"{}\"}}", escape_json_text(reason))
}

/// Replace the current process with a sandboxed command (Unix exec).
/// This function never returns on success — porta becomes the sandboxed process.
/// On failure, returns a JSON error string.
pub fn wt_exec_replace(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => replace_with_sandboxed(&request),
        Err(reason) => json_error(&reason),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn replace_with_sandboxed(_request: &SandboxRequest) -> String {
    "{\"error\":\"exec_replace not supported on this platform\"}".to_string()
}


/// Spawn a sandboxed command with this process's stdio, wait for it, and report
/// its exit code. Unlike `wt_exec_replace` porta stays alive, so a proxy thread
/// it started keeps serving, and it is still there to say what the sandbox
/// refused. Returns JSON: `{"exit_code":N}`, or `{"error":"..."}` with the
/// reason the run was refused before it started.
pub fn wt_exec_supervised(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => match request.supervise_with_snapshot() {
            Ok(code) => format!("{{\"exit_code\":{code}}}"),
            Err(reason) => json_error(&reason),
        },
        Err(reason) => json_error(&reason),
    }
}

impl SandboxRequest {
    /// The run, after a snapshot of its writable mounts when one was asked
    /// for, and then what it changed there.
    fn supervise_with_snapshot(&self) -> Result<i64, String> {
        if !self.snapshot {
            return supervise_sandboxed(self);
        }
        let mounts: Vec<String> = self.allowed_dirs.iter().filter(|dir| !dir.ends_with(":ro")).cloned().collect();
        let id = crate::snapshot::take(&mounts, &self.rerun_line())?;
        let code = supervise_sandboxed(self);
        eprint!("{}", crate::snapshot::after_run(&id, &mounts));
        code
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn supervise_sandboxed(_request: &SandboxRequest) -> Result<i64, String> {
    Err("sandboxed execution not supported on this platform".to_string())
}

/// Parse TOML through the maintained parser, preserving JSON-compatible values.
pub fn wt_parse_toml(content: impl AsRef<str>) -> String {
    match toml::from_str::<toml::Value>(content.as_ref()) {
        Ok(value) => serde_json::json!({"value": value}).to_string(),
        Err(error) => serde_json::json!({"error": error.to_string()}).to_string(),
    }
}

/// The copy of porta `--why` starts under `strace`: it applies the request
/// to itself and becomes the command, and returns only when that failed.
pub fn wt_exec_inner(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        #[cfg(target_os = "linux")]
        Ok(request) => why::exec_in_place(&request),
        #[cfg(not(target_os = "linux"))]
        Ok(_) => "running a request in place is only how --why works on Linux".to_string(),
        Err(reason) => reason,
    }
}
