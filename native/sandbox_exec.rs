//! Restricted execution of native commands.
//!
//! One parsed request drives three entry points — capture the output, replace
//! this process, or supervise a child — and each platform applies what it can
//! express, refusing the run when it cannot express a requested rule.

use crate::json_text::escape_json_text;
#[cfg(target_os = "linux")]
use crate::landlock_policy::readable_roots;
#[cfg(target_os = "macos")]
use crate::sandbox_profile::{build_sandbox_profile_rs, readable_roots};
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

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
}

fn open_reads() -> String { "open".to_string() }

/// Read policies this build understands.
const READ_POLICIES: [&str; 2] = ["open", "strict"];

/// Absolute, symlink-free mount path, keeping the `:ro` marker the policy
/// builders read. Both platforms match a rule against the path the kernel
/// resolved, so an absolute mount reached through a symlink — `/var/folders/…`,
/// which is really `/private/var/folders/…` on macOS — has to be named as the
/// kernel will see it, or the rule is written for a path nothing ever has.
fn resolve_mount(mount: &str) -> String {
    let clean = mount.trim_end_matches(":ro");
    let resolved = std::fs::canonicalize(clean)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| clean.to_string());
    if mount.ends_with(":ro") { format!("{}:ro", resolved) } else { resolved }
}

impl SandboxRequest {
    /// Reads one request document. A document that does not parse refuses the
    /// run: an empty request would grant nothing but would also say nothing.
    fn parse(request_json: &str) -> Result<Self, String> {
        let mut request: Self = serde_json::from_str(request_json)
            .map_err(|error| format!("invalid sandbox request: {error}"))?;
        if !READ_POLICIES.contains(&request.read_policy.as_str()) {
            return Err(format!("unknown read policy: {}; use open or strict", request.read_policy));
        }
        request.allowed_dirs = request.allowed_dirs.iter().map(|dir| resolve_mount(dir)).collect();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(reason) = request.unreadable_command() {
            return Err(reason);
        }
        Ok(request)
    }

    /// Why a strict read policy cannot start this command, if it cannot. A
    /// command porta may not read is a command it may not exec, and the kernel
    /// reports that as a bare `Permission denied` after the policy is already
    /// applied. The paths are still in hand here, so say what is wrong and
    /// which grant would fix it. A command named without a path is left to the
    /// `PATH` lookup: this is a diagnostic, and the enforcement stands either
    /// way.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn unreadable_command(&self) -> Option<String> {
        if self.read_policy != "strict" {
            return None;
        }
        let program = std::fs::canonicalize(&self.cmd).ok()?;
        let roots = readable_roots(&self.allowed_dirs);
        if roots.iter().any(|root| program.starts_with(root)) {
            return None;
        }
        Some(format!(
            "--read-policy strict leaves {} unreadable, so it cannot be started; \
             grant it with -v {} — a runtime that loads its own libraries needs \
             its whole install directory, not just this one",
            program.display(),
            program.parent()?.display(),
        ))
    }

    /// The profile this request asks for. All three macOS entry points build it
    /// here so none of them can apply a policy the other two do not.
    #[cfg(target_os = "macos")]
    fn profile(&self) -> String {
        build_sandbox_profile_rs(&self.allowed_dirs, &self.allowed_net, &self.read_policy)
    }

    /// The Landlock ruleset this request asks for, or why this kernel cannot
    /// apply it. Both Linux entry points come through here for the same reason
    /// the macOS ones come through [`Self::profile`].
    #[cfg(target_os = "linux")]
    fn ruleset(&self) -> Result<crate::landlock::Ruleset, String> {
        if self.proxy && !crate::seccomp::available() {
            return Err("proxy mode needs seccomp to deny UDP and Unix-socket egress, \
                        which this kernel will not accept; porta will not run the command \
                        with the rest of the policy applied".into());
        }
        crate::landlock_policy::ruleset(&self.allowed_dirs, &self.allowed_net, &self.read_policy)
    }

    /// Narrow the calling process to this request's policy. Runs after fork and
    /// before exec: two Landlock syscalls, then one more when the run claims
    /// the proxy is its only egress. Nothing here allocates.
    #[cfg(target_os = "linux")]
    fn restrict_current_process(descriptor: i32, proxy: bool) -> std::io::Result<()> {
        crate::landlock::Ruleset::restrict_current_process(descriptor)?;
        // Landlock's network rules reach TCP only. Everything else that could
        // carry bytes out is closed here, or the proxy is not the only egress.
        if proxy {
            crate::seccomp::restrict_current_process()?;
        }
        Ok(())
    }

    /// A command carrying this request's arguments, directory and environment.
    fn command(&self, program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        if !self.cwd.is_empty() && self.cwd != "." {
            command.current_dir(&self.cwd);
        }
        for (key, value) in &self.env_vars {
            command.env(key, value);
        }
        command
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

#[cfg(target_os = "macos")]
fn exec_sandboxed_macos(request: &SandboxRequest) -> String {
    let profile = request.profile();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    finish_sandboxed(command.output())
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

#[cfg(target_os = "linux")]
fn exec_sandboxed_linux(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;

    // Built before the fork so the child only has to apply it.
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    let ruleset_fd = ruleset.descriptor();
    let proxy = request.proxy;
    let mut command = request.command(&request.cmd);
    command.args(&request.args);
    unsafe {
        command.pre_exec(move || SandboxRequest::restrict_current_process(ruleset_fd, proxy));
    }
    finish_sandboxed(command.output())
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

#[cfg(target_os = "macos")]
fn replace_with_sandboxed(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;
    let profile = request.profile();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    // exec() replaces the current process — never returns on success
    json_error(&format!("exec failed: {}", command.exec()))
}

#[cfg(target_os = "linux")]
fn replace_with_sandboxed(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;
    // No fork here: porta restricts itself and then becomes the command. A
    // Landlock ruleset survives execve, so the restriction outlives this call.
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    if let Err(error) = SandboxRequest::restrict_current_process(ruleset.descriptor(), request.proxy) {
        return json_error(&format!("cannot apply the sandbox: {}", error));
    }
    let mut command = request.command(&request.cmd);
    command.args(&request.args);
    json_error(&format!("exec failed: {}", command.exec()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn replace_with_sandboxed(_request: &SandboxRequest) -> String {
    "{\"error\":\"exec_replace not supported on this platform\"}".to_string()
}

/// Spawn a sandboxed command with this process's stdio, wait for it, and report
/// its exit code. Unlike `wt_exec_replace` porta stays alive, so a proxy thread
/// it started keeps serving. Returns the exit code, or -1 when it cannot run.
pub fn wt_exec_supervised(request_json: impl AsRef<str>) -> i64 {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => supervise_sandboxed(&request),
        Err(_) => -1,
    }
}

#[cfg(target_os = "macos")]
fn supervise_sandboxed(request: &SandboxRequest) -> i64 {
    let profile = request.profile();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    command.stdin(std::process::Stdio::inherit());
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());
    match command.spawn().and_then(|mut child| child.wait()) {
        Ok(status) => status.code().unwrap_or(-1) as i64,
        Err(_) => -1,
    }
}

#[cfg(target_os = "linux")]
fn supervise_sandboxed(request: &SandboxRequest) -> i64 {
    use std::os::unix::process::CommandExt;

    // porta keeps its own sockets here — a proxy thread it started is still
    // serving — so only the child is narrowed, after the fork.
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => {
            eprintln!("[porta] {}", reason);
            return -1;
        }
    };
    let descriptor = ruleset.descriptor();
    let proxy = request.proxy;
    let mut command = request.command(&request.cmd);
    command.args(&request.args);
    command.stdin(std::process::Stdio::inherit());
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());
    unsafe {
        command.pre_exec(move || SandboxRequest::restrict_current_process(descriptor, proxy));
    }
    match command.spawn().and_then(|mut child| child.wait()) {
        Ok(status) => status.code().unwrap_or(-1) as i64,
        Err(_) => -1,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn supervise_sandboxed(_request: &SandboxRequest) -> i64 {
    -1
}

/// Parse TOML through the maintained parser, preserving JSON-compatible values.
pub fn wt_parse_toml(content: impl AsRef<str>) -> String {
    match toml::from_str::<toml::Value>(content.as_ref()) {
        Ok(value) => serde_json::json!({"value": value}).to_string(),
        Err(error) => serde_json::json!({"error": error.to_string()}).to_string(),
    }
}
