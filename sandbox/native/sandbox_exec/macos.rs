//! macOS enforcement for one request: the profile `sandbox-exec` applies, the
//! three ways a command is run under it, and the footer that says afterwards
//! what the kernel refused.

use super::*;
use std::os::unix::process::CommandExt;

impl SandboxRequest {
    /// The profile this request asks for. All three macOS entry points build it
    /// here so none of them can apply a policy the other two do not.
    #[cfg(target_os = "macos")]
    pub(super) fn profile(&self) -> String {
        build_sandbox_profile(&ProfileRequest {
            allowed_dirs: &self.allowed_dirs,
            allowed_net: &self.allowed_net,
            read_policy: &self.read_policy,
            proxy: self.proxy,
            no_network: self.no_network,
            bind_ports: &bind_ports(&self.allowed_bind).unwrap_or_default(),
            closures: &self.closures,
            allowed_unix: &self.allowed_unix,
            tag: &self.tag,
        })
    }
}

#[cfg(target_os = "macos")]
pub(super) fn exec_sandboxed_macos(request: &SandboxRequest) -> String {
    let profile = request.profile();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    finish_sandboxed(command.output())
}

#[cfg(target_os = "macos")]
pub(super) fn replace_with_sandboxed(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;
    let profile = request.profile();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    // exec() replaces the current process — never returns on success
    json_error(&format!("exec failed: {}", command.exec()))
}

#[cfg(target_os = "macos")]
pub(super) fn supervise_sandboxed(request: &SandboxRequest) -> Result<i64, String> {
    let profile = request.profile();
    let started = crate::denials::now_for_log();
    let mut command = request.command("sandbox-exec");
    command.arg("-p").arg(&profile).arg(&request.cmd).args(&request.args);
    command.stdin(std::process::Stdio::inherit());
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());
    // The child leads its own process group so `--timeout` can signal the
    // whole tree, not just the sandbox-exec shell in front of the command.
    command.process_group(0);
    let code = command
        .spawn()
        .map_err(|error| format!("cannot start the command: {error}"))
        .and_then(|child| {
            wait_within(child, request.timeout, request.max_cpu, request.max_memory_mb.saturating_mul(MIB))
                .map_err(|error| format!("waiting for the command failed: {error}"))
        })?;
    explain_denials(request, &started, code);
    Ok(code)
}

/// After a run, say what the sandbox refused and what would have allowed it.
/// A run that succeeded is not questioned unless asked (`PORTA_DENIALS=always`):
/// the log query costs most of a second, and a tool that met a refusal and
/// carried on chose to. `PORTA_DENIALS=never` keeps the footer away entirely.
#[cfg(target_os = "macos")]
pub(super) fn explain_denials(request: &SandboxRequest, started: &str, code: i64) {
    use crate::ceilings::{CPU_EXCEEDED, MEMORY_EXCEEDED, TIMED_OUT};
    // --why asks for the footer whatever the outcome, as PORTA_DENIALS=always does.
    let setting = if request.why { "always".to_string() } else { std::env::var("PORTA_DENIALS").unwrap_or_default() };
    // A run porta's own supervisor ended has nothing the kernel refused to
    // explain, and the log query would cost it seconds of retries.
    let ended_by_porta = matches!(code, TIMED_OUT | CPU_EXCEEDED | MEMORY_EXCEEDED);
    if setting == "never" || ((code == 0 || ended_by_porta) && setting != "always") {
        return;
    }
    let denials = crate::denials::collect(&request.tag, started);
    if denials.is_empty() && request.why {
        eprintln!("[porta] the sandbox refused nothing this run");
    }
    eprint!("{}", crate::denials::footer(&denials, &request.rerun_line(), &request.closures));
}
