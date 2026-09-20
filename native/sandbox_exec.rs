//! Restricted execution of native commands.
//!
//! One parsed request drives three entry points — capture the output, replace
//! this process, or supervise a child — and each platform applies what it can
//! express, refusing the run when it cannot express a requested rule.

use crate::json_text::escape_json_text;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

/// One sandboxed execution request, parsed once and shared by both entry points.
struct SandboxRequest {
    cmd: String,
    args: Vec<String>,
    allowed_dirs: Vec<String>,
    allowed_net: Vec<String>,
    env_vars: Vec<(String, String)>,
    cwd: String,
}

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

fn parse_env_pairs(env_json: &str) -> Vec<(String, String)> {
    serde_json::from_str::<Vec<Vec<String>>>(env_json)
        .unwrap_or_default()
        .into_iter()
        .filter(|pair| pair.len() == 2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

impl SandboxRequest {
    fn parse(
        cmd: &str,
        args_json: &str,
        allowed_dirs_json: &str,
        allowed_net_json: &str,
        env_json: &str,
        cwd: &str,
    ) -> Self {
        let raw_dirs: Vec<String> = serde_json::from_str(allowed_dirs_json).unwrap_or_default();
        SandboxRequest {
            cmd: cmd.to_string(),
            args: serde_json::from_str(args_json).unwrap_or_default(),
            allowed_dirs: raw_dirs.iter().map(|dir| resolve_mount(dir)).collect(),
            allowed_net: serde_json::from_str(allowed_net_json).unwrap_or_default(),
            env_vars: parse_env_pairs(env_json),
            cwd: cwd.to_string(),
        }
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
pub fn wt_exec_sandboxed(
    cmd: impl AsRef<str>,
    args_json: impl AsRef<str>,
    allowed_dirs_json: impl AsRef<str>,
    allowed_net_json: impl AsRef<str>,
    env_json: impl AsRef<str>,
    cwd: impl AsRef<str>,
) -> String {
    let request = SandboxRequest::parse(
        cmd.as_ref(), args_json.as_ref(), allowed_dirs_json.as_ref(),
        allowed_net_json.as_ref(), env_json.as_ref(), cwd.as_ref(),
    );
    run_sandboxed(&request)
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

/// Writable roots the macOS profile grants on every run. `/tmp` is reached
/// through `/private/tmp` there, so the profile has to name both spellings.
const PROFILE_WRITABLE: [&str; 3] = ["/tmp", "/private/tmp", "/dev"];

/// The same roots as [`PROFILE_WRITABLE`], as Linux spells them.
#[cfg(target_os = "linux")]
const ALWAYS_WRITABLE: [&str; 2] = ["/tmp", "/dev"];

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

/// The Landlock policy a request asks for, or why this kernel cannot apply it.
#[cfg(target_os = "linux")]
fn linux_ruleset(request: &SandboxRequest) -> Result<crate::landlock::Ruleset, String> {
    let policy = crate::landlock::Policy {
        writable_dirs: writable_dirs(&request.allowed_dirs),
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
pub fn wt_exec_replace(
    cmd: impl AsRef<str>,
    args_json: impl AsRef<str>,
    allowed_dirs_json: impl AsRef<str>,
    allowed_net_json: impl AsRef<str>,
    env_json: impl AsRef<str>,
    cwd: impl AsRef<str>,
) -> String {
    let request = SandboxRequest::parse(
        cmd.as_ref(), args_json.as_ref(), allowed_dirs_json.as_ref(),
        allowed_net_json.as_ref(), env_json.as_ref(), cwd.as_ref(),
    );
    replace_with_sandboxed(&request)
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

fn sandbox_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Shared sandbox profile builder for Rust-side exec functions.
fn build_sandbox_profile_rs(allowed_dirs: &[String], allowed_net: &[String]) -> String {
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

/// Spawn a sandboxed command, inherit stdio, wait, and return the exit code.
/// Unlike wt_exec_replace, this keeps the porta process alive so it can supervise
/// a concurrent proxy thread. Returns exit code (>=0) or -1 on spawn failure.
pub fn wt_exec_supervised(
    cmd: impl AsRef<str>,
    args_json: impl AsRef<str>,
    allowed_dirs_json: impl AsRef<str>,
    allowed_net_json: impl AsRef<str>,
    env_json: impl AsRef<str>,
    cwd: impl AsRef<str>,
) -> i64 {
    let args: Vec<String> = serde_json::from_str(args_json.as_ref()).unwrap_or_default();
    let allowed_dirs_raw: Vec<String> =
        serde_json::from_str(allowed_dirs_json.as_ref()).unwrap_or_default();
    let allowed_dirs: Vec<String> = allowed_dirs_raw
        .iter()
        .map(|d| {
            let clean = d.trim_end_matches(":ro");
            let abs = if clean.starts_with('/') {
                clean.to_string()
            } else {
                std::fs::canonicalize(clean)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| clean.to_string())
            };
            if d.ends_with(":ro") { format!("{}:ro", abs) } else { abs }
        })
        .collect();
    let allowed_net: Vec<String> = serde_json::from_str(allowed_net_json.as_ref()).unwrap_or_default();
    let env_vars: Vec<(String, String)> =
        serde_json::from_str::<Vec<Vec<String>>>(env_json.as_ref())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pair| {
                if pair.len() == 2 { Some((pair[0].clone(), pair[1].clone())) } else { None }
            })
            .collect();

    #[cfg(target_os = "macos")]
    {
        let profile = build_sandbox_profile_rs(&allowed_dirs, &allowed_net);
        let mut command = std::process::Command::new("sandbox-exec");
        command.arg("-p").arg(&profile).arg(cmd.as_ref()).args(&args);
        if !cwd.as_ref().is_empty() && cwd.as_ref() != "." {
            command.current_dir(cwd.as_ref());
        }
        for (k, v) in &env_vars {
            command.env(k, v);
        }
        command.stdin(std::process::Stdio::inherit());
        command.stdout(std::process::Stdio::inherit());
        command.stderr(std::process::Stdio::inherit());

        match command.spawn() {
            Ok(mut child) => match child.wait() {
                Ok(status) => status.code().unwrap_or(-1) as i64,
                Err(_) => -1,
            },
            Err(_) => -1,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (allowed_dirs, allowed_net, env_vars);
        -1
    }
}

/// Parse TOML through the maintained parser, preserving JSON-compatible values.
pub fn wt_parse_toml(content: impl AsRef<str>) -> String {
    match toml::from_str::<toml::Value>(content.as_ref()) {
        Ok(value) => serde_json::json!({"value": value}).to_string(),
        Err(error) => serde_json::json!({"error": error.to_string()}).to_string(),
    }
}

pub fn wt_sandbox_profile(dirs_json: impl AsRef<str>, net_json: impl AsRef<str>) -> String {
    match (serde_json::from_str::<Vec<String>>(dirs_json.as_ref()), serde_json::from_str::<Vec<String>>(net_json.as_ref())) {
        (Ok(dirs), Ok(net)) => build_sandbox_profile_rs(&dirs, &net),
        _ => "(version 1)\n(deny default)\n".to_string(),
    }
}
