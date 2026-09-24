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
}

/// A tag for one run: the pid and the clock, which no two runs on one host
/// share. It is a label for log lines, not a secret.
fn run_tag() -> String {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut now) };
    format!("{:x}{:x}", std::process::id(), now.tv_nsec)
}

/// Host variables a child keeps. Everything else the caller's shell holds —
/// API keys, tokens, the SSH agent's socket — stays outside unless `-e` or
/// `--env-pass` names it. A locale, a terminal and a path are what a command
/// needs to start; a credential is not. `TMPDIR` is left out on purpose: the
/// sandbox's temporary directory is `/tmp`, the one it is granted.
const INHERITED_ENV: [&str; 10] =
    ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "COLORTERM", "LANG", "LANGUAGE", "TZ"];

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

/// What the child applies to itself between fork and exec. Plain data, so the
/// closure that carries it into `pre_exec` copies and allocates nothing.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct ChildPolicy {
    ruleset: i32,
    /// Which seccomp program closes the channels Landlock cannot see.
    egress: crate::seccomp::Egress,
}

fn open_reads() -> String { "open".to_string() }

/// Read policies this build understands.
const READ_POLICIES: [&str; 2] = ["open", "strict"];

/// Absolute, symlink-free mount path, keeping the `:ro` marker the policy
/// builders read. Both platforms match a rule against the path the kernel
/// resolved, so an absolute mount reached through a symlink — `/var/folders/…`,
/// which is really `/private/var/folders/…` on macOS — has to be named as the
/// kernel will see it, or the rule is written for a path nothing ever has.
///
/// A mount that does not exist is refused rather than passed through as
/// written. Passed through, it reached the kernel as a relative path and the
/// run failed later with an exec error that named nothing the caller typed.
fn resolve_mount(mount: &str) -> Result<String, String> {
    let clean = mount.strip_suffix(":ro").unwrap_or(mount);
    let resolved = std::fs::canonicalize(clean)
        .map_err(|error| format!("mount {clean} cannot be used: {error}"))?;
    if !resolved.is_dir() {
        return Err(format!("mount {clean} is not a directory; -v takes a directory to grant"));
    }
    let resolved = resolved.to_string_lossy().to_string();
    Ok(if mount.ends_with(":ro") { format!("{}:ro", resolved) } else { resolved })
}

/// Why this command cannot be started at all, if it cannot: it is not a
/// path that exists, and not a name on the PATH porta itself was started
/// with. Found here, before any policy is applied, so the answer names the
/// command rather than the exec wrapper that failed to find it.
/// The file the kernel will execute for `cmd`, resolved: a path as given
/// (relative to the run's directory), or the first `PATH` entry holding it,
/// the way the shell would find it. `None` when nothing resolves.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn resolve_command(cmd: &str, cwd: &str) -> Option<std::path::PathBuf> {
    if cmd.contains('/') {
        let base = if cwd.is_empty() { "." } else { cwd };
        return std::fs::canonicalize(std::path::Path::new(base).join(cmd)).ok();
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(cmd)).find(|candidate| candidate.is_file()).and_then(|found| std::fs::canonicalize(found).ok())
}

fn missing_command(cmd: &str, cwd: &str) -> Option<String> {
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
    /// Reads one request document. A document that does not parse refuses the
    /// run: an empty request would grant nothing but would also say nothing.
    fn parse(request_json: &str) -> Result<Self, String> {
        let mut request: Self = serde_json::from_str(request_json)
            .map_err(|error| format!("invalid sandbox request: {error}"))?;
        request.tag = run_tag();
        if !READ_POLICIES.contains(&request.read_policy.as_str()) {
            return Err(format!("unknown read policy: {}; use open or strict", request.read_policy));
        }
        if let Some(reason) = request.running_as_root() {
            return Err(reason);
        }
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

    /// Network flags that contradict each other, if any do.
    fn contradictory_network(&self) -> Option<&'static str> {
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
    fn memory_ceiling_unavailable(&self) -> Option<String> {
        if self.max_memory_mb == 0 {
            return None;
        }
        crate::memory_ceiling::unavailable().map(|reason| format!("--max-memory-mb: {reason}"))
    }

    /// macOS has no cgroup; the supervisor measures the group's footprint and
    /// ends it at the ceiling, so the flag is honoured, in that sense.
    #[cfg(target_os = "macos")]
    fn memory_ceiling_unavailable(&self) -> Option<String> {
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn memory_ceiling_unavailable(&self) -> Option<String> {
        (self.max_memory_mb > 0).then(|| "--max-memory-mb has no enforcement on this platform".to_string())
    }

    /// The ceilings this request sets, in the kernel's units. The file size
    /// is taken in MiB because a byte count is not a number anyone types.
    fn ceilings(&self) -> Vec<Ceiling> {
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

    /// The ceilings in words, for `explain`.
    fn explain_ceilings(&self) -> String {
        let mut parts = Vec::new();
        if self.max_cpu > 0 {
            parts.push(format!("{}s CPU per process", self.max_cpu));
        }
        if self.max_procs > 0 {
            parts.push(format!("{} processes for this user", self.max_procs));
        }
        if self.max_file_size > 0 {
            parts.push(format!("files up to {} MiB", self.max_file_size));
        }
        if self.max_memory_mb > 0 {
            let how = if cfg!(target_os = "linux") { "cgroup, swap closed" } else { "the group is ended when its footprint reaches it" };
            parts.push(format!("{} MiB resident memory for the whole run ({how})", self.max_memory_mb));
        }
        if parts.is_empty() { "none".to_string() } else { parts.join(", ") }
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
    fn running_as_root(&self) -> Option<String> {
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
    fn unreadable_command(&self) -> Option<String> {
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

    /// The profile this request asks for. All three macOS entry points build it
    /// here so none of them can apply a policy the other two do not.
    #[cfg(target_os = "macos")]
    fn profile(&self) -> String {
        build_sandbox_profile(&ProfileRequest {
            allowed_dirs: &self.allowed_dirs,
            allowed_net: &self.allowed_net,
            read_policy: &self.read_policy,
            proxy: self.proxy,
            no_network: self.no_network,
            bind_ports: &bind_ports(&self.allowed_bind).unwrap_or_default(),
            allowed_unix: &self.allowed_unix,
            tag: &self.tag,
        })
    }

    /// The policy in words, for `porta explain`: what the kernel will be told,
    /// and what the child will see. Nothing runs.
    fn explain(&self) -> String {
        let mut text = String::new();
        text.push_str(&format!("command      {} {}\n", self.cmd, self.args.join(" ")));
        text.push_str(&format!("working dir  {}\n", if self.cwd.is_empty() { "." } else { &self.cwd }));
        text.push_str("mounts       ");
        text.push_str(&if self.allowed_dirs.is_empty() { "none".to_string() } else { self.allowed_dirs.join(", ") });
        text.push('\n');
        text.push_str(&format!("reads        {}\n", if self.read_policy == "strict" { "mounts and the platform's own directories only" } else { "open, minus credential stores" }));
        text.push_str(&format!(
            "network      {}\n",
            if self.no_network { "none".to_string() } else if self.proxy { "the loopback proxy only".to_string() } else if self.allowed_net.is_empty() { "open".to_string() } else { format!("TCP to {}", self.allowed_net.join(", ")) }
        ));
        if !self.allowed_bind.is_empty() {
            text.push_str(&format!("listen on    {}\n", self.allowed_bind.join(", ")));
        }
        if !self.allowed_unix.is_empty() {
            text.push_str(&format!("unix sockets {}\n", self.allowed_unix.join(", ")));
        }
        text.push_str(&format!(
            "time limit   {}\n",
            if self.timeout == 0 { "none".to_string() } else { format!("{}s, then killed with its process group", self.timeout) }
        ));
        text.push_str(&format!("resources    {}\n", self.explain_ceilings()));
        let mut inherited: Vec<&str> = INHERITED_ENV.iter().copied().filter(|key| std::env::var_os(key).is_some()).collect();
        let named: Vec<&str> = self.env_vars.iter().map(|(key, _)| key.as_str()).collect();
        inherited.extend(named.iter().copied());
        text.push_str(&format!("environment  {}\n", inherited.join(" ")));
        text.push_str(&self.explain_enforcement());
        text
    }

    /// The same policy as `explain`, as one JSON object for tooling and CI. A
    /// refused request is not reached here: the wrapper reports the refusal.
    fn explain_json(&self) -> String {
        let quoted = |text: &str| format!("\"{}\"", crate::json_text::escape_json_text(text));
        let array = |items: &[String]| -> String {
            let parts: Vec<String> = items
                .iter()
                .map(|item| format!("\"{}\"", crate::json_text::escape_json_text(item)))
                .collect();
            format!("[{}]", parts.join(","))
        };
        let (net_mode, net_allow): (&str, &[String]) = if self.no_network {
            ("none", &[])
        } else if self.proxy {
            ("proxy", &[])
        } else if self.allowed_net.is_empty() {
            ("open", &[])
        } else {
            ("allowlist", &self.allowed_net)
        };
        format!(
            "{{\"command\":{},\"args\":{},\"working_dir\":{},\"mounts\":{},\"reads\":{},\
\"network\":{{\"mode\":{},\"allow\":{}}},\"listen\":{},\"unix_sockets\":{},\
\"timeout_seconds\":{},\"limits\":{{\"cpu_seconds\":{},\"processes\":{},\"file_size_mib\":{},\"memory_mib\":{}}},\
\"enforcement\":{}}}",
            quoted(&self.cmd),
            array(&self.args),
            quoted(if self.cwd.is_empty() { "." } else { &self.cwd }),
            array(&self.allowed_dirs),
            quoted(if self.read_policy == "strict" { "strict" } else { "open" }),
            quoted(net_mode),
            array(net_allow),
            array(&self.allowed_bind),
            array(&self.allowed_unix),
            self.timeout,
            self.max_cpu,
            self.max_procs,
            self.max_file_size,
            self.max_memory_mb,
            quoted(self.enforcement_backend()),
        )
    }

    #[cfg(target_os = "macos")]
    fn enforcement_backend(&self) -> &'static str {
        "sandbox-exec"
    }

    #[cfg(target_os = "linux")]
    fn enforcement_backend(&self) -> &'static str {
        "landlock+seccomp"
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn enforcement_backend(&self) -> &'static str {
        "none"
    }

    #[cfg(target_os = "macos")]
    fn explain_enforcement(&self) -> String {
        format!("\nsandbox-exec profile:\n{}", self.profile())
    }

    #[cfg(target_os = "linux")]
    fn explain_enforcement(&self) -> String {
        let seccomp = match self.egress() {
            crate::seccomp::Egress::ProxyOnly => "baseline plus: socket() only for AF_INET SOCK_STREAM",
            _ => "baseline: ptrace, process_vm_*, mounts, namespaces, io_uring, raw/packet/vsock, MPTCP refused",
        };
        let ruleset = match self.ruleset() {
            Ok(_) => "this kernel can express every rule above".to_string(),
            Err(reason) => format!("this kernel cannot: {reason}"),
        };
        format!("\nLandlock      {ruleset}\nseccomp       {seccomp}\n")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn explain_enforcement(&self) -> String {
        "\nno enforcement backend on this platform; every native run is refused\n".to_string()
    }

    /// Which seccomp program this request needs beside Landlock.
    #[cfg(target_os = "linux")]
    fn egress(&self) -> crate::seccomp::Egress {
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
    fn ruleset(&self) -> Result<crate::landlock::Ruleset, String> {
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
            connect: &self.allowed_net,
            bind: &bind_ports(&self.allowed_bind)?,
            // Under --no-net without a namespace of its own, Landlock closes
            // every TCP port. With one, nothing but the command's own loopback
            // is there to reach, and a test suite may serve on it.
            closed: self.no_network && !self.network_isolated(),
        };
        crate::landlock_policy::ruleset(&self.allowed_dirs, network, &self.read_policy)
    }

    /// Whether this run's network is a namespace of its own.
    #[cfg(target_os = "linux")]
    fn network_isolated(&self) -> bool {
        self.no_network && crate::pid_namespace::available().is_ok()
    }

    /// Everything the child applies to itself, gathered before the fork.
    #[cfg(target_os = "linux")]
    fn child_policy(&self, ruleset: &crate::landlock::Ruleset) -> ChildPolicy {
        ChildPolicy { ruleset: ruleset.descriptor(), egress: self.egress() }
    }

    /// Narrow the calling process to this request's policy. Runs after fork and
    /// before exec: the Landlock syscalls, then the seccomp filter that closes
    /// what Landlock cannot see. Nothing here allocates.
    #[cfg(target_os = "linux")]
    fn restrict_current_process(policy: ChildPolicy) -> std::io::Result<()> {
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
    fn spawned_command(&self, isolation: Option<crate::pid_namespace::Isolation>) -> std::process::Command {
        use std::os::unix::process::CommandExt;
        let stub = self.max_memory_mb > 0 && isolation.is_none();
        let mut command = self.bare_command(if stub { "/bin/sh" } else { &self.cmd });
        if let Some(isolation) = isolation {
            unsafe {
                command.pre_exec(move || isolation.enter());
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
    /// without them and, when `/proc` is otherwise readable, says so once.
    #[cfg(target_os = "linux")]
    fn isolation(&self) -> Result<(Option<crate::pid_namespace::Isolation>, Option<crate::pid_namespace::Gate>), String> {
        use crate::pid_namespace::{available, Isolation};
        match available() {
            Ok(()) => Isolation::prepare(self.max_memory_mb > 0, self.no_network)
                .map(|(isolation, gate)| (Some(isolation), gate))
                .map_err(|error| format!("cannot prepare the command's namespaces: {error}")),
            Err(reason) => {
                static NOTED: std::sync::Once = std::sync::Once::new();
                if self.read_policy != "strict" {
                    NOTED.call_once(|| {
                        eprintln!("porta: {reason}, so the command shares the host's process list; --read-policy strict closes /proc");
                    });
                }
                Ok((None, None))
            }
        }
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
    fn command(&self, program: &str) -> std::process::Command {
        let mut command = self.bare_command(program);
        with_ceilings(&mut command, self.ceilings());
        command
    }

    /// `command` without the ceilings, for a caller that must put a step of
    /// its own after the fork first.
    fn bare_command(&self, program: &str) -> std::process::Command {
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
        for (key, value) in &self.env_vars {
            command.env(key, value);
        }
        command
    }
}

pub use crate::http_proxy::{wt_is_host_allowed, wt_proxy_start, wt_proxy_stop};

/// Execute a command inside an OS-level sandbox.
/// Returns JSON: {"exit_code":0,"stdout":"...","stderr":"..."} or {"error":"..."}
fn with_ceilings(command: &mut std::process::Command, ceilings: Vec<Ceiling>) {
    use std::os::unix::process::CommandExt;
    if !ceilings.is_empty() {
        unsafe {
            command.pre_exec(move || apply_ceilings(&ceilings));
        }
    }
}

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
    if let Err(error) = SandboxRequest::restrict_current_process(request.child_policy(&ruleset)) {
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

/// The policy a request would apply, in words, without applying it. A request
/// porta would refuse explains the refusal instead.
pub fn wt_sandbox_explain(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => request.explain(),
        Err(reason) => format!("porta would refuse this run: {reason}\n"),
    }
}

/// The policy as one JSON object, or `{"refused":"..."}` when porta would
/// refuse the run before it began.
pub fn wt_sandbox_explain_json(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => request.explain_json(),
        Err(reason) => format!("{{\"refused\":\"{}\"}}", crate::json_text::escape_json_text(&reason)),
    }
}

/// Spawn a sandboxed command with this process's stdio, wait for it, and report
/// its exit code. Unlike `wt_exec_replace` porta stays alive, so a proxy thread
/// it started keeps serving, and it is still there to say what the sandbox
/// refused. Returns JSON: `{"exit_code":N}`, or `{"error":"..."}` with the
/// reason the run was refused before it started.
pub fn wt_exec_supervised(request_json: impl AsRef<str>) -> String {
    match SandboxRequest::parse(request_json.as_ref()) {
        Ok(request) => match supervise_sandboxed(&request) {
            Ok(code) => format!("{{\"exit_code\":{code}}}"),
            Err(reason) => json_error(&reason),
        },
        Err(reason) => json_error(&reason),
    }
}

#[cfg(target_os = "macos")]
fn supervise_sandboxed(request: &SandboxRequest) -> Result<i64, String> {
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
    explain_denials(&request.tag, &started, code, &request.rerun_line());
    Ok(code)
}

/// After a run, say what the sandbox refused and what would have allowed it.
/// A run that succeeded is not questioned unless asked (`PORTA_DENIALS=always`):
/// the log query costs most of a second, and a tool that met a refusal and
/// carried on chose to. `PORTA_DENIALS=never` keeps the footer away entirely.
#[cfg(target_os = "macos")]
fn explain_denials(tag: &str, started: &str, code: i64, rerun: &str) {
    use crate::ceilings::{CPU_EXCEEDED, MEMORY_EXCEEDED, TIMED_OUT};
    let setting = std::env::var("PORTA_DENIALS").unwrap_or_default();
    // A run porta's own supervisor ended has nothing the kernel refused to
    // explain, and the log query would cost it seconds of retries.
    let ended_by_porta = matches!(code, TIMED_OUT | CPU_EXCEEDED | MEMORY_EXCEEDED);
    if setting == "never" || ((code == 0 || ended_by_porta) && setting != "always") {
        return;
    }
    let denials = crate::denials::collect(tag, started);
    eprint!("{}", crate::denials::footer(&denials, rerun));
}

/// `word` as a shell would need it typed: bare when it is plain, in single
/// quotes otherwise.
#[cfg(target_os = "macos")]
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_./:=@%+,".contains(ch));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

#[cfg(target_os = "macos")]
impl SandboxRequest {
    /// The `porta run` command line that would reproduce this request, for
    /// the footer to add grants to. Options are rebuilt from the policy rather
    /// than remembered, so the order is porta's, not the caller's.
    fn rerun_line(&self) -> String {
        let mut line = format!("porta run {}", self.cmd);
        for dir in &self.allowed_dirs {
            line.push_str(&format!(" -v {dir}"));
        }
        if self.proxy {
            line.push_str(" --proxy-allow <hosts>");
        } else {
            for net in &self.allowed_net {
                line.push_str(&format!(" --allow-net '{net}'"));
            }
        }
        for port in &self.allowed_bind {
            line.push_str(&format!(" --allow-bind {port}"));
        }
        for path in &self.allowed_unix {
            line.push_str(&format!(" --allow-unix {path}"));
        }
        if self.read_policy == "strict" {
            line.push_str(" --read-policy strict");
        }
        if !self.args.is_empty() {
            line.push_str(" -- ");
            line.push_str(&self.args.iter().map(|arg| shell_word(arg)).collect::<Vec<_>>().join(" "));
        }
        line
    }
}

#[cfg(target_os = "linux")]
fn supervise_sandboxed(request: &SandboxRequest) -> Result<i64, String> {
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
    wait_within(child, request.timeout, request.max_cpu, 0).map_err(|error| format!("waiting for the command failed: {error}"))
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
