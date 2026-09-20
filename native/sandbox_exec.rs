//! Restricted execution of native commands.
//!
//! One parsed request drives three entry points — capture the output, replace
//! this process, or supervise a child — and each platform applies what it can
//! express, refusing the run when it cannot express a requested rule.

use crate::json_text::escape_json_text;
use crate::sandbox_profile::build_sandbox_profile_rs;
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
}

fn open_reads() -> String { "open".to_string() }

/// Read policies this build understands.
const READ_POLICIES: [&str; 2] = ["open", "strict"];

/// Absolute mount path, keeping the `:ro` marker the policy builders read.
fn resolve_mount(mount: &str) -> String {
    let clean = mount.trim_end_matches(":ro");
    let absolute = if clean.starts_with('/') {
        clean.to_string()
    } else {
        std::fs::canonicalize(clean)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| clean.to_string())
    };
    if mount.ends_with(":ro") { format!("{}:ro", absolute) } else { absolute }
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
        // macOS cannot express a strict read policy yet. `(deny file-read*)`
        // with a system allow-list aborts every process on macOS 26.3, and the
        // profile trace facility that would say what dyld needs is itself
        // denied. Refusing keeps the guarantee honest, as the network and proxy
        // paths already do on Linux.
        #[cfg(target_os = "macos")]
        if request.read_policy == "strict" {
            return Err("--read-policy strict is not implemented on macOS; \
                        porta will not run the command with reads open instead".into());
        }
        request.allowed_dirs = request.allowed_dirs.iter().map(|dir| resolve_mount(dir)).collect();
        Ok(request)
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
    let profile = build_sandbox_profile_rs(&request.allowed_dirs, &request.allowed_net);
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

/// The same roots as [`crate::sandbox_profile::PROFILE_WRITABLE`], as Linux
/// spells them: there is no `/private/tmp` to name.
#[cfg(target_os = "linux")]
const ALWAYS_WRITABLE: [&str; 2] = ["/tmp", "/dev"];

/// The platform's own directories, readable under a strict read policy. A
/// dynamically linked command cannot start without its interpreter, its
/// libraries and the loader cache, so confining reads to the granted mounts
/// alone would only mean nothing runs. A caller's home directory is
/// deliberately absent: that is what this policy exists to close.
#[cfg(target_os = "linux")]
const SYSTEM_READABLE: [&str; 7] = ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc"];

/// TCP ports from `--allow-net` entries, or the entry that cannot be expressed.
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
fn readable_dirs(allowed_dirs: &[String]) -> Vec<String> {
    allowed_dirs.iter().map(|dir| dir.trim_end_matches(":ro").to_string()).collect()
}

/// The Landlock policy a request asks for, or why this kernel cannot apply it.
#[cfg(target_os = "linux")]
fn linux_ruleset(request: &SandboxRequest) -> Result<crate::landlock::Ruleset, String> {
    let strict = request.read_policy == "strict";
    let policy = crate::landlock::Policy {
        writable_dirs: writable_dirs(&request.allowed_dirs),
        readable_dirs: if strict { readable_dirs(&request.allowed_dirs) } else { Vec::new() },
        system_dirs: SYSTEM_READABLE.iter().map(|dir| dir.to_string()).collect(),
        restrict_reads: strict,
        tcp_ports: requested_tcp_ports(&request.allowed_net)?,
        restrict_network: !request.allowed_net.is_empty(),
    };
    crate::landlock::prepare(&policy)
}

#[cfg(target_os = "linux")]
fn exec_sandboxed_linux(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;

    // Built before the fork so the child only has to apply it.
    let ruleset = match linux_ruleset(request) {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    let ruleset_fd = ruleset.descriptor();
    let mut command = request.command(&request.cmd);
    command.args(&request.args);
    // Runs after fork and before exec: two syscalls, no allocation.
    unsafe {
        command.pre_exec(move || crate::landlock::Ruleset::restrict_current_process(ruleset_fd));
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
    let profile = build_sandbox_profile_rs(&request.allowed_dirs, &request.allowed_net);
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
    let ruleset = match linux_ruleset(request) {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    if let Err(error) = crate::landlock::Ruleset::restrict_current_process(ruleset.descriptor()) {
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
    let profile = build_sandbox_profile_rs(&request.allowed_dirs, &request.allowed_net);
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

/// Proxy mode needs egress narrowed to one loopback endpoint with UDP and Unix
/// sockets denied. Landlock expresses none of that, so the run is refused here
/// rather than supervised with part of the policy missing.
#[cfg(not(target_os = "macos"))]
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
