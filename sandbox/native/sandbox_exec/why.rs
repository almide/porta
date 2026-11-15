//! `--why` on Linux: what the sandbox refused during a run, and which flag
//! would have allowed it.
//!
//! macOS logs every Seatbelt denial; Linux logs Landlock's only to the audit
//! subsystem, which an unprivileged process cannot read. So under `--why`
//! porta runs the command under `strace`, which traces from outside the
//! sandbox and records only the calls that failed, and reads the refusals
//! back from them. A call the file permissions refused is not the sandbox's
//! doing: porta asks the same question itself, outside, and keeps only what
//! it is allowed and the command was not.

use super::*;
use crate::denial_advice::Denial;
use std::collections::{BTreeSet, HashMap};

/// How many refusals the footer lists before it says how many more there are.
const SHOWN: usize = 25;

/// The syscalls that create, remove or rename a path: the last path they name
/// is the one written. `symlink`'s first argument is the link's contents.
const WRITES: [&str; 14] = [
    "mkdir", "mkdirat", "unlink", "unlinkat", "rmdir", "rename", "renameat", "renameat2", "link", "linkat", "symlink",
    "symlinkat", "mknodat", "truncate",
];

/// The syscalls that look a path up without writing it.
const READS: [&str; 12] = [
    "execve", "execveat", "stat", "lstat", "newfstatat", "statx", "access", "faccessat", "faccessat2", "readlink",
    "readlinkat", "chdir",
];

/// The flags `strace` is started with: every process the command starts,
/// only the calls that failed, each descriptor with the path it names, and
/// the file and network calls alone.
const STRACE_ARGS: [&str; 9] = ["-f", "-qq", "-Z", "-y", "-s", "4096", "-e", "trace=%file,%network", "-o"];

impl SandboxRequest {
    /// Runs the request under `strace`, then prints what the sandbox refused.
    /// The traced process is porta itself, started again to apply this very
    /// request to itself and become the command, so the policy is the one an
    /// untraced run gets and `strace` stays outside it.
    pub(super) fn supervise_traced(&self) -> Result<i64, String> {
        use std::os::unix::process::CommandExt;
        if self.max_memory_mb > 0 {
            return Err("--why cannot be combined with --max-memory-mb on Linux: the ceiling is placed around the command, and under --why strace stands in front of it".into());
        }
        let strace = super::checks::resolve_command("strace", "").ok_or(
            "--why on Linux watches the run through strace, and strace is not on the PATH; install it (apt install strace) and run again",
        )?;
        let porta = std::env::current_exe().map_err(|error| format!("--why cannot find porta's own binary: {error}"))?;
        let log = std::env::temp_dir().join(format!("porta-why-{}.log", self.tag));
        let sockets = self.closed_sockets();
        let mut command = std::process::Command::new(strace);
        command.args(STRACE_ARGS).arg(&log).arg("--").arg(porta).arg(INNER_COMMAND).arg(&self.raw);
        command.process_group(0);
        let child = command.spawn().map_err(|error| format!("cannot start strace for --why: {error}"))?;
        let code = wait_within(child, self.timeout, self.max_cpu, 0).map_err(|error| format!("waiting for the command failed: {error}"));
        let trace = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_file(&log);
        let denials = refusals(&trace, &sockets);
        eprint!("{}", self.why_footer(&denials));
        code
    }

    fn why_footer(&self, denials: &[Denial]) -> String {
        if denials.is_empty() {
            return "[porta] the sandbox refused nothing this run\n".to_string();
        }
        let mut text = crate::denial_advice::footer(&denials[..denials.len().min(SHOWN)], &self.rerun_line(), &self.closures);
        if denials.len() > SHOWN {
            text.push_str(&format!("  … and {} more\n", denials.len() - SHOWN));
        }
        text
    }
}

/// After a failed run at a terminal, one line on how to learn whether the
/// sandbox was the cause: Linux keeps no record of what it refused unless the
/// run is traced, which `--why` does. `PORTA_DENIALS=never` silences it, as
/// it silences the macOS footer.
pub(super) fn point_at_why(code: i64) {
    use crate::ceilings::{CPU_EXCEEDED, MEMORY_EXCEEDED, TIMED_OUT};
    let at_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let ended_by_porta = matches!(code, TIMED_OUT | CPU_EXCEEDED | MEMORY_EXCEEDED);
    if code != 0 && !ended_by_porta && at_terminal && std::env::var("PORTA_DENIALS").as_deref() != Ok("never") {
        eprintln!("[porta] exit {code}; run again with --why to see anything the sandbox refused");
    }
}

/// Applies the request to this very process and becomes the command: the
/// namespaces, the ceilings and the policy, as a spawned run applies them
/// after its fork. Returns only when that failed.
pub(super) fn exec_in_place(request: &SandboxRequest) -> String {
    use std::os::unix::process::CommandExt;
    let ruleset = match request.ruleset() {
        Ok(ruleset) => ruleset,
        Err(reason) => return reason,
    };
    let policy = request.child_policy(&ruleset);
    let isolation = match request.isolation() {
        Ok((isolation, _)) => isolation,
        Err(reason) => return reason,
    };
    let mut command = request.spawned_command(isolation);
    unsafe {
        command.pre_exec(move || SandboxRequest::restrict_current_process(policy));
    }
    format!("cannot start the command under the sandbox: {}", command.exec())
}

/// The argument porta is started with under `strace` to run one request in
/// place; not a command anyone types.
pub(crate) const INNER_COMMAND: &str = "__sandboxed";

/// The refusals in one `strace` log, each once. `sockets` are the credential
/// sockets this run covered: a refused connection to one of them is the
/// sandbox's, where any other refused connection is only a closed port.
pub(super) fn refusals(trace: &str, sockets: &[String]) -> Vec<Denial> {
    let mut found = BTreeSet::new();
    for call in calls(trace) {
        if let Some(denial) = refusal(&call, sockets) {
            found.insert(denial);
        }
    }
    found.into_iter().collect()
}

/// One failed call: its name, its arguments as `strace` printed them, and
/// the errno it returned.
struct Call {
    name: String,
    args: String,
    errno: String,
}

/// The failed calls in a log, with a call `strace` split across two lines
/// (`<unfinished ...>`, then `<... name resumed>`) put back together.
fn calls(trace: &str) -> Vec<Call> {
    let mut pending: HashMap<&str, String> = HashMap::new();
    let mut calls = Vec::new();
    for line in trace.lines() {
        let Some((pid, rest)) = line.split_once(' ') else { continue };
        let rest = rest.trim_start();
        if let Some(start) = rest.strip_suffix(" <unfinished ...>") {
            pending.insert(pid, start.to_string());
            continue;
        }
        let whole = match rest.strip_prefix("<... ").and_then(|resumed| resumed.split_once(" resumed>")) {
            Some((_, tail)) => format!("{}{tail}", pending.remove(pid).unwrap_or_default()),
            None => rest.to_string(),
        };
        if let Some(call) = parse_call(&whole) {
            calls.push(call);
        }
    }
    calls
}

fn parse_call(line: &str) -> Option<Call> {
    let (name, rest) = line.split_once('(')?;
    let (args, result) = rest.rsplit_once(") = -1 ")?;
    let errno = result.split_whitespace().next()?.to_string();
    Some(Call { name: name.to_string(), args: args.to_string(), errno })
}

fn refusal(call: &Call, sockets: &[String]) -> Option<Denial> {
    let name = call.name.as_str();
    match name {
        "connect" | "bind" | "socket" => network_refusal(call, sockets),
        _ if !matches!(call.errno.as_str(), "EACCES" | "EPERM" | "EROFS" | "EBUSY") => None,
        "open" | "openat" | "openat2" | "creat" => {
            let path = paths(&call.args).into_iter().next()?;
            let writes = name == "creat" || ["O_WRONLY", "O_RDWR", "O_CREAT", "O_TRUNC"].iter().any(|flag| call.args.contains(flag));
            file_refusal(if writes { "file-write-data" } else { "file-read-data" }, path)
        }
        _ if WRITES.contains(&name) => file_refusal("file-write-create", paths(&call.args).pop()?),
        _ if READS.contains(&name) && call.errno == "EACCES" => file_refusal("file-read-metadata", paths(&call.args).into_iter().next()?),
        _ => None,
    }
}

/// A refused file call, unless porta itself, outside the sandbox, would be
/// refused the same: then the file permissions refused it, not the policy.
fn file_refusal(operation: &str, path: String) -> Option<Denial> {
    if porta_own(&path) {
        return None;
    }
    let writes = operation.starts_with("file-write");
    let asked = if writes && !std::path::Path::new(&path).exists() {
        std::path::Path::new(&path).parent().map(|parent| parent.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        path.clone()
    };
    let asked = std::ffi::CString::new(asked).ok()?;
    let mode = if writes { libc::W_OK } else { libc::R_OK };
    // Only a question about a path; nothing is opened.
    let allowed_outside = unsafe { libc::access(asked.as_ptr(), mode) } == 0;
    allowed_outside.then(|| Denial { operation: operation.to_string(), target: path })
}

/// A path porta itself writes before the command exists: the maps of the
/// user namespace it tries to give the command. Where the host refuses that
/// namespace the write fails in the traced process, and it is porta's, not
/// the command's.
fn porta_own(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/proc/") else { return false };
    let (process, file) = rest.split_once('/').unwrap_or((rest, ""));
    let a_process = process == "self" || process == "thread-self" || process.bytes().all(|byte| byte.is_ascii_digit());
    a_process && matches!(file, "setgroups" | "uid_map" | "gid_map")
}

/// A refused TCP connect or bind (Landlock's EACCES), a refused connection to
/// one of the credential sockets this run covered, or a socket kind seccomp
/// refused.
fn network_refusal(call: &Call, sockets: &[String]) -> Option<Denial> {
    if call.name == "socket" {
        // seccomp's answer to a family or kind no port rule speaks for
        let refused = matches!(call.errno.as_str(), "EAFNOSUPPORT" | "EPROTONOSUPPORT");
        let kind: Vec<&str> = call.args.splitn(3, ", ").take(2).collect();
        return refused.then(|| Denial { operation: "network-socket".into(), target: kind.join(" ") });
    }
    if let Some(path) = quoted_field(&call.args, "sun_path=") {
        let covered = sockets.iter().any(|socket| *socket == path || std::fs::canonicalize(&path).is_ok_and(|real| real.to_string_lossy() == *socket));
        return covered.then(|| Denial { operation: "network-outbound".into(), target: path });
    }
    if call.errno != "EACCES" {
        return None;
    }
    let port = call.args.split("_port=htons(").nth(1)?.split(')').next()?.to_string();
    Some(match call.name.as_str() {
        "bind" => Denial { operation: "network-bind".into(), target: format!("local:*:{port}") },
        _ => Denial { operation: "network-outbound".into(), target: format!("remote:*:{port}") },
    })
}

/// The paths a call names, each made absolute against the directory `strace`
/// printed beside it (`AT_FDCWD</cwd>`, `3</dir>`).
fn paths(args: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut base = String::new();
    let mut rest = args;
    while let Some(start) = rest.find(['"', '<']) {
        let open = rest.as_bytes()[start];
        rest = &rest[start + 1..];
        if open == b'<' {
            let end = rest.find('>').unwrap_or(rest.len());
            base = rest[..end].to_string();
            rest = &rest[end..];
            continue;
        }
        let (text, after) = unquote(rest);
        rest = after;
        found.push(if text.starts_with('/') || base.is_empty() { text } else { format!("{base}/{text}") });
        base.clear();
    }
    found
}

/// The string `strace` printed after `field`, unquoted.
fn quoted_field(args: &str, field: &str) -> Option<String> {
    let start = args.find(field)? + field.len();
    args[start..].strip_prefix('"').map(|rest| unquote(rest).0)
}

/// A C string as `strace` quotes it, up to its closing quote, and what
/// follows. `\"` and `\\` are the escapes a path is likely to hold; others
/// are kept as printed.
fn unquote(text: &str) -> (String, &str) {
    let mut out = String::new();
    let mut chars = text.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return (out, &text[index + 1..]),
            '\\' => match chars.next() {
                Some((_, escaped @ ('"' | '\\'))) => out.push(escaped),
                Some((_, other)) => {
                    out.push('\\');
                    out.push(other);
                }
                None => break,
            },
            _ => out.push(ch),
        }
    }
    (out, "")
}

#[cfg(test)]
mod tests;
