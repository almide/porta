//! What refuses a request before any policy is applied: a mount or command
//! that is not there, flags that contradict each other, a ceiling the host
//! cannot place, root, and a strict read policy that could not start the command.

use super::*;

/// `path` with a leading `~/` made the caller's home.
pub(super) fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// Absolute, symlink-free mount path, keeping the `:ro` marker the policy
/// builders read. Both platforms match a rule against the path the kernel
/// resolved, so an absolute mount reached through a symlink — `/var/folders/…`,
/// which is really `/private/var/folders/…` on macOS — has to be named as the
/// kernel will see it, or the rule is written for a path nothing ever has.
///
/// A mount that does not exist is refused rather than passed through as
/// written. Passed through, it reached the kernel as a relative path and the
/// run failed later with an exec error that named nothing the caller typed.
///
/// `~/` is the caller's home, as in a shell and as `porta.toml` examples write
/// it; a quoted `-v '~/x'` and a TOML string reach porta unexpanded.
pub(super) fn resolve_mount(mount: &str) -> Result<String, String> {
    let clean = mount.strip_suffix(":ro").unwrap_or(mount);
    let resolved = std::fs::canonicalize(expand_home(clean))
        .map_err(|error| format!("mount {clean} cannot be used: {error}"))?;
    if !resolved.is_dir() {
        return Err(format!("mount {clean} is not a directory; -v takes a directory to grant"));
    }
    let resolved = resolved.to_string_lossy().to_string();
    Ok(if mount.ends_with(":ro") { format!("{}:ro", resolved) } else { resolved })
}

/// The file the kernel will execute for `cmd`, resolved: a path as given
/// (relative to the run's directory), or the first `PATH` entry holding it,
/// the way the shell would find it. `None` when nothing resolves.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn resolve_command(cmd: &str, cwd: &str) -> Option<std::path::PathBuf> {
    if cmd.contains('/') {
        let base = if cwd.is_empty() { "." } else { cwd };
        return std::fs::canonicalize(std::path::Path::new(base).join(cmd)).ok();
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(cmd)).find(|candidate| candidate.is_file()).and_then(|found| std::fs::canonicalize(found).ok())
}


/// Why this command cannot be started at all, if it cannot: it is not a
/// path that exists, and not a name on the PATH porta itself was started
/// with. Found here, before any policy is applied, so the answer names the
/// command rather than the exec wrapper that failed to find it.
pub(super) fn missing_command(cmd: &str, cwd: &str) -> Option<String> {
    if cmd.contains('/') {
        let base = if cwd.is_empty() { "." } else { cwd };
        let program = std::path::Path::new(base).join(cmd);
        return (!program.exists()).then(|| format!("command not found: {cmd}"));
    }
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path).any(|dir| dir.join(cmd).is_file());
    (!found).then(|| format!("command not found: {cmd} (not on PATH)"))
}

impl SandboxRequest {
    /// Network flags that contradict each other, if any do.
    pub(super) fn contradictory_network(&self) -> Option<&'static str> {
        if self.no_network && (self.proxy || !self.allowed_net.is_empty() || !self.allowed_bind.is_empty()) {
            return Some("--no-net closes the network; it cannot also grant --allow-net, --allow-bind or a proxy");
        }
        (!self.allowed_bind.is_empty() && self.allowed_net.is_empty()).then_some(
            "--allow-bind only means something once --allow-net closes the network; \
             with the network open every port can already be bound",
        )
    }

    /// Why `--max-memory-mb` cannot be honoured here, if it cannot. A ceiling
    /// this host cannot enforce refuses the run, like any other rule.
    #[cfg(target_os = "linux")]
    pub(super) fn memory_ceiling_unavailable(&self) -> Option<String> {
        if self.max_memory_mb == 0 {
            return None;
        }
        crate::memory_ceiling::unavailable().map(|reason| format!("--max-memory-mb: {reason}"))
    }

    /// macOS has no cgroup; the supervisor measures the group's footprint and
    /// ends it at the ceiling, so the flag is honoured, in that sense.
    #[cfg(target_os = "macos")]
    pub(super) fn memory_ceiling_unavailable(&self) -> Option<String> {
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub(super) fn memory_ceiling_unavailable(&self) -> Option<String> {
        (self.max_memory_mb > 0).then(|| "--max-memory-mb has no enforcement on this platform".to_string())
    }

    /// Why this run is refused for being root, if it is.
    ///
    /// Half of what keeps a confined command away from a secret is file
    /// permissions, not this policy. `/etc` has to be readable for anything to
    /// start, and it carries `shadow` and host keys beside the `ld.so.cache`
    /// and `ssl/certs` a command genuinely needs; for an ordinary user those
    /// are separated by their mode bits, and for root they are not separated
    /// at all. porta would be claiming a confinement it does not have, so it
    /// refuses instead — the same answer it gives a rule the kernel cannot
    /// express.
    ///
    /// A container image whose only user is root is a real place to run this,
    /// so `--allow-root` proceeds. It is not a flag anything sets by default:
    /// the caller has to have decided that permissions are not part of the
    /// boundary they wanted.
    pub(super) fn running_as_root(&self) -> Option<String> {
        if self.allow_root || unsafe { libc::geteuid() } != 0 {
            return None;
        }
        Some(
            "refusing to run as root: file permissions are part of what keeps a confined \
             command away from a secret, and for root they separate nothing — /etc must be \
             readable for a command to start, and it holds shadow and host keys. Run as an \
             ordinary user, or pass --allow-root if you have decided permissions are not \
             part of the boundary you wanted."
                .to_string(),
        )
    }

    /// Why a strict read policy cannot start this command, if it cannot. A
    /// command porta may not read is a command it may not exec, and the kernel
    /// reports that as a bare `Permission denied` after the policy is already
    /// applied. The paths are still in hand here, so say what is wrong and
    /// which grant would fix it. A command named without a path is left to the
    /// `PATH` lookup: this is a diagnostic, and the enforcement stands either
    /// way.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn unreadable_command(&self) -> Option<String> {
        if self.read_policy != "strict" {
            return None;
        }
        let program = resolve_command(&self.cmd, &self.cwd)?;
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
}
