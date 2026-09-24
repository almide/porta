//! The command a request runs: an environment that starts empty, the run's
//! directory, and the resource ceilings set between fork and exec.

use super::*;

/// Host variables a child keeps. Everything else the caller's shell holds —
/// API keys, tokens, the SSH agent's socket — stays outside unless `-e` or
/// `--env-pass` names it. A locale, a terminal and a path are what a command
/// needs to start; a credential is not. `TMPDIR` is left out on purpose: the
/// sandbox's temporary directory is `/tmp`, the one it is granted.
pub(super) const INHERITED_ENV: [&str; 10] =
    ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "COLORTERM", "LANG", "LANGUAGE", "TZ"];

impl SandboxRequest {
    /// The ceilings this request sets, in the kernel's units. The file size
    /// is taken in MiB because a byte count is not a number anyone types.
    pub(super) fn ceilings(&self) -> Vec<Ceiling> {
        [
            (libc::RLIMIT_CPU as libc::c_int, self.max_cpu, "--max-cpu"),
            (libc::RLIMIT_NPROC as libc::c_int, self.max_procs, "--max-procs"),
            (libc::RLIMIT_FSIZE as libc::c_int, self.max_file_size.saturating_mul(MIB), "--max-file-size"),
        ]
        .into_iter()
        .filter(|(_, value, _)| *value > 0)
        .map(|(resource, value, flag)| Ceiling { resource, value, flag })
        .collect()
    }

    /// A command carrying this request's arguments, directory and environment.
    ///
    /// The environment starts empty. The host variables a command needs to run
    /// are copied over by name, then the caller's `-e` values; nothing else of
    /// the caller's shell crosses into the sandbox.
    ///
    /// The resource ceilings are set here too, after the fork and before the
    /// exec, so every path that runs a command — spawned, supervised or
    /// replacing porta itself — applies them the same way.
    pub(super) fn command(&self, program: &str) -> std::process::Command {
        let mut command = self.bare_command(program);
        with_ceilings(&mut command, self.ceilings());
        command
    }

    /// `command` without the ceilings, for a caller that must put a step of
    /// its own after the fork first.
    pub(super) fn bare_command(&self, program: &str) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command.env_clear();
        for key in INHERITED_ENV {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        for (key, value) in std::env::vars().filter(|(key, _)| key.starts_with("LC_")) {
            command.env(key, value);
        }
        if !self.cwd.is_empty() && self.cwd != "." {
            command.current_dir(&self.cwd);
        }
        #[cfg(target_os = "linux")]
        if self.egress() == crate::seccomp::Egress::TcpPorts {
            command.env(RESOLVER_OVER_TCP.0, RESOLVER_OVER_TCP.1);
        }
        for (key, value) in &self.env_vars {
            command.env(key, value);
        }
        command
    }
}

pub(super) fn with_ceilings(command: &mut std::process::Command, ceilings: Vec<Ceiling>) {
    use std::os::unix::process::CommandExt;
    if !ceilings.is_empty() {
        unsafe {
            command.pre_exec(move || apply_ceilings(&ceilings));
        }
    }
}
