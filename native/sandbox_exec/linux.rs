//! Linux enforcement for one request: the Landlock ruleset and seccomp program
//! built before the fork, the namespaces and covers entered after it, and the
//! three ways a command is run under them.

use super::*;

/// Under `--allow-net` on Linux, UDP is closed and a resolver asks over TCP:
/// glibc reads its options from this variable as well as `resolv.conf`, and Go
/// hands lookups to glibc when it is set. `-e` can override it.
#[cfg(target_os = "linux")]
pub(super) const RESOLVER_OVER_TCP: (&str, &str) = ("RES_OPTIONS", "use-vc");

/// What the child applies to itself between fork and exec. Plain data, so the
/// closure that carries it into `pre_exec` copies and allocates nothing.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub(super) struct ChildPolicy {
    pub(super) ruleset: i32,
    /// Which seccomp program closes the channels Landlock cannot see.
    pub(super) egress: crate::seccomp::Egress,
}

impl SandboxRequest {
    /// Which seccomp program this request needs beside Landlock.
    #[cfg(target_os = "linux")]
    pub(super) fn egress(&self) -> crate::seccomp::Egress {
        // Without a namespace of its own, no network is the proxy filter with
        // no port Landlock lets TCP reach: nothing leaves.
        if self.proxy || (self.no_network && !self.network_isolated()) {
            crate::seccomp::Egress::ProxyOnly
        } else if !self.allowed_net.is_empty() {
            crate::seccomp::Egress::TcpPorts
        } else {
            crate::seccomp::Egress::Open
        }
    }

    /// The Landlock ruleset this request asks for, or why this kernel cannot
    /// apply it. Both Linux entry points come through here for the same reason
    /// the macOS ones come through [`Self::profile`].
    #[cfg(target_os = "linux")]
    pub(super) fn ruleset(&self) -> Result<crate::landlock::Ruleset, String> {
        let abi = crate::landlock::abi_version().unwrap_or(0);
        if self.no_network && !self.network_isolated() && abi < 4 {
            return Err(format!(
                "--no-net needs a network namespace or Landlock ABI 4, and this host gives neither ({}; Landlock ABI {abi}); \
                 porta will not run the command with the network open",
                crate::pid_namespace::available().err().unwrap_or("")
            ));
        }
        if !crate::seccomp::available() {
            return Err("this kernel will not accept a seccomp filter, and porta closes the \
                        syscalls Landlock cannot see with one; porta will not run the command \
                        with the rest of the policy applied"
                .into());
        }
        // Built here, in the parent, so the child has only to point at it.
        crate::seccomp::prepare(self.egress());
        let network = crate::landlock_policy::Network {
            connect: &self.connect_ports(),
            bind: &bind_ports(&self.allowed_bind)?,
            // Under --no-net without a namespace of its own, Landlock closes
            // every TCP port. With one, nothing but the command's own loopback
            // is there to reach, and a test suite may serve on it.
            closed: self.no_network && !self.network_isolated(),
        };
        // With a mount namespace the command's own, the closed paths are
        // covered there; without one, Landlock has to leave them out.
        let closed: &[String] = if crate::pid_namespace::available().is_ok() { &[] } else { &self.closures.deny_read };
        crate::landlock_policy::ruleset(&self.allowed_dirs, network, &self.read_policy, closed)
    }

    /// The credential sockets bound on this host now that the preset closes
    /// and `--allow-unix` does not open. The patterns were checked at parse.
    #[cfg(target_os = "linux")]
    pub(super) fn closed_sockets(&self) -> Vec<String> {
        crate::unix_sockets::closed(&self.closures.deny_unix, &self.allowed_unix, &self.closures.deny_read).unwrap_or_default()
    }

    /// The ports `--allow-net` names, plus TCP 53 once UDP is closed: name
    /// resolution then goes over TCP (see [`RESOLVER_OVER_TCP`]).
    #[cfg(target_os = "linux")]
    pub(super) fn connect_ports(&self) -> Vec<String> {
        let mut ports = self.allowed_net.clone();
        if self.egress() == crate::seccomp::Egress::TcpPorts {
            ports.push("*:53".to_string());
        }
        ports
    }

    /// Whether this run's network is a namespace of its own.
    #[cfg(target_os = "linux")]
    pub(super) fn network_isolated(&self) -> bool {
        self.no_network && crate::pid_namespace::available().is_ok()
    }

    /// Everything the child applies to itself, gathered before the fork.
    #[cfg(target_os = "linux")]
    pub(super) fn child_policy(&self, ruleset: &crate::landlock::Ruleset) -> ChildPolicy {
        ChildPolicy { ruleset: ruleset.descriptor(), egress: self.egress() }
    }

    /// Narrow the calling process to this request's policy. Runs after fork and
    /// before exec: the Landlock syscalls, then the seccomp filter that closes
    /// what Landlock cannot see. Nothing here allocates.
    #[cfg(target_os = "linux")]
    pub(super) fn restrict_current_process(policy: ChildPolicy) -> std::io::Result<()> {
        crate::landlock::Ruleset::restrict_current_process(policy.ruleset)?;
        crate::seccomp::restrict_current_process(policy.egress)
    }

    /// The command as spawned, in its own namespaces when `isolation` is given
    /// (see `pid_namespace`); entering them is the first step after the fork,
    /// before the ceilings and the policy.
    ///
    /// Under a memory ceiling without the namespaces it starts as a shell that
    /// stops itself and then becomes the command: the parent's `spawn` returns
    /// only once something has exec'd, so a stop before exec would deadlock it,
    /// while a stop after exec of a stub leaves the same pid — the one porta
    /// places in the scope — to `exec` the real command once continued. The
    /// stub runs under the same Landlock and seccomp policy as the command.
    /// With the namespaces the outermost process waits at a gate instead,
    /// before any of the command exists.
    #[cfg(target_os = "linux")]
    pub(super) fn spawned_command(&self, isolation: Option<crate::pid_namespace::Isolation>) -> std::process::Command {
        use std::os::unix::process::CommandExt;
        let stub = self.max_memory_mb > 0 && isolation.is_none();
        let mut command = self.bare_command(if stub { "/bin/sh" } else { &self.cmd });
        if let Some(isolation) = isolation {
            let writable: Vec<String> = self.allowed_dirs.iter().filter(|dir| !dir.ends_with(":ro")).cloned().collect();
            let hidden = crate::pid_namespace::Hidden::prepare(&self.closures, &self.closed_sockets(), &writable);
            unsafe {
                command.pre_exec(move || isolation.enter(&hidden));
            }
        }
        // Inside the namespaces the kernel counts the namespace's processes
        // against --max-procs, porta's two helpers among them.
        let helpers = if isolation.is_some() { crate::pid_namespace::HELPER_PROCESSES } else { 0 };
        let ceilings: Vec<Ceiling> = self
            .ceilings()
            .into_iter()
            .map(|ceiling| if ceiling.resource == libc::RLIMIT_NPROC as libc::c_int { Ceiling { value: ceiling.value + helpers, ..ceiling } } else { ceiling })
            .collect();
        with_ceilings(&mut command, ceilings);
        if stub {
            command.args(["-c", "kill -STOP $$ && exec \"$0\" \"$@\"", &self.cmd]);
        }
        command.args(&self.args);
        command
    }

    /// The namespaces for this run where the host gives them, with the gate
    /// porta holds under a memory ceiling. Where it does not, the run goes
    /// without them and says once what that leaves open: `/proc` when it is
    /// otherwise readable, and a writable mount's hooks and protected names.
    #[cfg(target_os = "linux")]
    pub(super) fn isolation(&self) -> Result<(Option<crate::pid_namespace::Isolation>, Option<crate::pid_namespace::Gate>), String> {
        use crate::pid_namespace::{available, Isolation};
        match available() {
            Ok(()) => Isolation::prepare(self.max_memory_mb > 0, self.no_network)
                .map(|(isolation, gate)| (Some(isolation), gate))
                .map_err(|error| format!("cannot prepare the command's namespaces: {error}")),
            Err(reason) => {
                static NOTED: std::sync::Once = std::sync::Once::new();
                let mut open = Vec::new();
                if self.read_policy != "strict" {
                    open.push("the command shares the host's process list (--read-policy strict closes /proc)".to_string());
                }
                if self.allowed_dirs.iter().any(|dir| !dir.ends_with(":ro")) {
                    open.push("a writable mount's protected names and repository files stay writable".to_string());
                }
                let sockets = self.closed_sockets();
                if !sockets.is_empty() {
                    open.push(format!("these sockets stay reachable: {}", sockets.join(", ")));
                }
                if !open.is_empty() {
                    NOTED.call_once(|| eprintln!("porta: {reason}, so {}", open.join("; ")));
                }
                Ok((None, None))
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn exec_sandboxed_linux(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;

    // Built before the fork so the child only has to apply it.
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    let policy = request.child_policy(&ruleset);
    let isolation = match request.isolation() {
        Ok((isolation, _)) => isolation,
        Err(reason) => return json_error(&reason),
    };
    let mut command = request.spawned_command(isolation);
    unsafe {
        command.pre_exec(move || SandboxRequest::restrict_current_process(policy));
    }
    finish_sandboxed(command.output())
}

#[cfg(target_os = "linux")]
pub(super) fn replace_with_sandboxed(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;
    // No fork here: porta restricts itself and then becomes the command. A
    // Landlock ruleset survives execve, so the restriction outlives this call.
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => return json_error(&reason),
    };
    if let Err(error) = SandboxRequest::restrict_current_process(request.child_policy(&ruleset)) {
        return json_error(&format!("cannot apply the sandbox: {}", error));
    }
    let mut command = request.command(&request.cmd);
    command.args(&request.args);
    json_error(&format!("exec failed: {}", command.exec()))
}

#[cfg(target_os = "linux")]
pub(super) fn supervise_sandboxed(request: &SandboxRequest) -> Result<i64, String> {
    if request.why {
        return request.supervise_traced();
    }
    use std::os::unix::process::CommandExt;

    // porta keeps its own sockets here — a proxy thread it started is still
    // serving — so only the child is narrowed, after the fork.
    let ruleset = request.ruleset()?;
    let policy = request.child_policy(&ruleset);
    let (isolation, gate) = request.isolation()?;
    let mut command = request.spawned_command(isolation);
    command.stdin(std::process::Stdio::inherit());
    command.stdout(std::process::Stdio::inherit());
    command.stderr(std::process::Stdio::inherit());
    unsafe {
        command.pre_exec(move || SandboxRequest::restrict_current_process(policy));
    }
    // Its own process group, so `--timeout` reaches everything the command
    // starts, not only the command itself.
    command.process_group(0);
    // With the namespaces, the ceiling is placed while `spawn` is still
    // waiting: the outermost child stops at the gate before the command
    // exists, and `spawn` returns only once the command has exec'd.
    let bytes = request.max_memory_mb.saturating_mul(MIB);
    let (placing, child_ends) = match gate {
        Some(gate) => (Some(crate::memory_ceiling::place_at_gate(gate.ready, gate.go, request.tag.clone(), bytes)), Some(gate.child_ends)),
        None => (None, None),
    };
    let spawned = command.spawn();
    // The child has its own copies now; porta's must go, or a child that died
    // before using them would never read as gone.
    drop(child_ends);
    let placed = placing.map(|thread| thread.join().unwrap_or_else(|_| Err("--max-memory-mb: placing the command panicked".into())));
    let mut child = match (spawned, placed) {
        (Ok(mut child), Some(Err(reason))) => {
            crate::ceilings::kill_group(&mut child);
            return Err(reason);
        }
        (Err(_), Some(Err(reason))) => return Err(reason),
        (spawned, _) => spawned.map_err(|error| format!("cannot start the command under the sandbox: {error}"))?,
    };
    if request.max_memory_mb > 0 && isolation.is_none() {
        // The child has stopped itself after applying its policy; place it
        // under the ceiling, or end it there — never let it run without one.
        crate::memory_ceiling::place(&mut child, &request.tag, request.max_memory_mb.saturating_mul(MIB))?;
    }
    // The memory ceiling is the cgroup's here, so the supervisor watches none.
    let code = wait_within(child, request.timeout, request.max_cpu, 0).map_err(|error| format!("waiting for the command failed: {error}"))?;
    super::why::point_at_why(code);
    Ok(code)
}
