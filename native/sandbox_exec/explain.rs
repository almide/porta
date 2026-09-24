//! What a request would do, in words and as JSON, for `porta explain`, and
//! the command line that reproduces it, for the denial footer.

use super::*;

impl SandboxRequest {
    /// The ceilings in words, for `explain`.
    pub(super) fn explain_ceilings(&self) -> String {
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

    /// The policy in words, for `porta explain`: what the kernel will be told,
    /// and what the child will see. Nothing runs.
    pub(super) fn explain(&self) -> String {
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
        text.push_str(&self.explain_closures());
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

    /// What the preset and the caller close, in words.
    pub(super) fn explain_closures(&self) -> String {
        let preset = if self.preset.is_empty() { "default" } else { &self.preset };
        let closures = &self.closures;
        let mut text = format!("preset       {preset} (--preset none drops it; --deny-read, --protect, --deny-unix add)\n");
        text.push_str(&format!("closed reads {}\n", if closures.deny_read.is_empty() { "none".to_string() } else { closures.deny_read.join("\n             ") }));
        text.push_str(&format!("protected    {}\n", if closures.protect.is_empty() { "none".to_string() } else { closures.protect.join(", ") }));
        text.push_str(&format!("closed unix  {}\n", if closures.deny_unix.is_empty() { "none".to_string() } else { closures.deny_unix.join("  ") }));
        #[cfg(target_os = "linux")]
        text.push_str(&format!("  bound now  {}\n", self.closed_sockets().join("\n             ")));
        text
    }

    /// The same policy as `explain`, as one JSON object for tooling and CI. A
    /// refused request is not reached here: the wrapper reports the refusal.
    pub(super) fn explain_json(&self) -> String {
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
\"preset\":{},\"closed\":{{\"read\":{},\"protect\":{},\"unix\":{}}},\"enforcement\":{}}}",
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
            quoted(if self.preset.is_empty() { "default" } else { &self.preset }),
            array(&self.closures.deny_read),
            array(&self.closures.protect),
            array(&self.closures.deny_unix),
            quoted(self.enforcement_backend()),
        )
    }

    #[cfg(target_os = "macos")]
    pub(super) fn enforcement_backend(&self) -> &'static str {
        "sandbox-exec"
    }

    #[cfg(target_os = "linux")]
    pub(super) fn enforcement_backend(&self) -> &'static str {
        "landlock+seccomp"
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn enforcement_backend(&self) -> &'static str {
        "none"
    }

    #[cfg(target_os = "macos")]
    pub(super) fn explain_enforcement(&self) -> String {
        format!("\nsandbox-exec profile:\n{}", self.profile())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn explain_enforcement(&self) -> String {
        let seccomp = match self.egress() {
            crate::seccomp::Egress::ProxyOnly => "baseline plus: socket() only for AF_INET SOCK_STREAM",
            crate::seccomp::Egress::TcpPorts => "baseline plus: an AF_INET/AF_INET6 socket only as TCP (no UDP; names resolve over TCP 53)",
            _ => "baseline: ptrace, process_vm_*, mounts, namespaces, io_uring, raw/packet/vsock, MPTCP refused",
        };
        let ruleset = match self.ruleset() {
            Ok(_) => "this kernel can express every rule above".to_string(),
            Err(reason) => format!("this kernel cannot: {reason}"),
        };
        format!("\nLandlock      {ruleset}\nseccomp       {seccomp}\n")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub(super) fn explain_enforcement(&self) -> String {
        "\nno enforcement backend on this platform; every native run is refused\n".to_string()
    }
}

/// `word` as a shell would need it typed: bare when it is plain, in single
/// quotes otherwise.
pub(super) fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_./:=@%+,".contains(ch));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

impl SandboxRequest {
    /// The `porta run` command line that would reproduce this request, for
    /// the footer to add grants to. Options are rebuilt from the policy rather
    /// than remembered, so the order is porta's, not the caller's.
    pub(super) fn rerun_line(&self) -> String {
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
