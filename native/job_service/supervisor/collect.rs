//! Turning a finished worker into the end of its record: why it stopped, what
//! it used, what it printed and what it left in `/output`.

use super::*;

const MAX_OUTPUT_FILES: usize = 256;
const MAX_OUTPUT_DEPTH: usize = 8;
const INLINE_TEXT: usize = 64 * 1024;

/// How the worker process ended, as the supervisor saw it.
pub(super) struct Ended {
    pub(super) status: Option<std::process::ExitStatus>,
    pub(super) killed_for: Option<&'static str>,
    pub(super) wall: Duration,
}

/// The reason and the sentence for it. A stop the supervisor asked for wins
/// over whatever the worker managed to write first.
pub(super) fn stop_reason(ended: &Ended, result: Option<&WorkerResult>, workspace: &Path, os_sandbox: bool) -> (String, String) {
    match (ended.killed_for, result) {
        (Some("timeout"), _) => ("timeout".into(), "the worker passed its deadline and was killed by the supervisor".into()),
        (Some(reason), _) => (reason.into(), format!("stopped by the supervisor: {reason}")),
        (None, Some(r)) => (r.stop_reason.clone(), r.detail.clone()),
        // `porta run` exits 125 when it refuses to start the command.
        (None, None) if os_sandbox && ended.status.and_then(|s| s.code()) == Some(125) =>
            ("sandbox_refused".into(), format!("the OS sandbox refused to start the worker: {}", log_tail(workspace))),
        (None, None) => ("worker_crashed".into(), format!("the worker ended without a result ({})", describe(ended.status))),
    }
}

pub(super) fn usage(result: Option<&WorkerResult>, wall: Duration) -> Value {
    json!({
        // An interrupted store does not report the fuel it burned; say so
        // rather than print a number that looks exact.
        "fuel_consumed": result.filter(|r| r.stop_reason != "timeout").map(|r| r.fuel_consumed),
        "peak_memory_bytes": result.map(|r| r.peak_memory_bytes).unwrap_or(0),
        "memory_grow_denied": result.is_some_and(|r| r.memory_denied),
        "wall_ms": wall.as_millis() as u64,
    })
}

/// How a stop reason reads as an outcome.
pub(super) fn outcome(reason: &str, exit_code: Option<i64>) -> &'static str {
    match reason {
        "exit" if exit_code == Some(0) => "succeeded",
        "timeout" | "fuel_exhausted" | "memory_limit" | "output_limit" | "cancelled" | "service_shutdown" | "interrupted" => "stopped",
        "link_refused" | "module_changed" | "sandbox_refused" => "refused",
        _ => "failed",
    }
}

/// The last line porta wrote about the worker, never guest output: the
/// guest's streams are in memory pipes, not on the worker's stderr.
fn log_tail(workspace: &Path) -> String {
    let text = std::fs::read_to_string(workspace.join("worker.log")).unwrap_or_default();
    text.trim().lines().last().unwrap_or("no message").chars().take(500).collect()
}

fn describe(status: Option<std::process::ExitStatus>) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status {
        None => "unknown status".into(),
        Some(status) => match status.signal() {
            Some(signal) => format!("signal {signal}"),
            None => format!("exit {}", status.code().unwrap_or(-1)),
        },
    }
}

/// A guest stream as the record holds it: always its size and digest, whether
/// it reached its cap, and its text only when the policy retains output.
pub(super) fn stream(path: &Path, keep: Option<PathBuf>, cap: u64) -> Value {
    let bytes = std::fs::read(path).unwrap_or_default();
    // At the cap, the guest's further writes failed; what is here is a prefix.
    let mut entry = json!({"bytes": bytes.len(), "sha256": sha256_hex(&bytes), "capped": bytes.len() as u64 >= cap});
    if let Some(keep) = keep {
        if let Some(parent) = keep.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(&keep, &bytes);
        let shown = &bytes[..bytes.len().min(INLINE_TEXT)];
        entry["text"] = json!(String::from_utf8_lossy(shown));
        entry["truncated"] = json!(bytes.len() > INLINE_TEXT);
    }
    entry
}

/// What the guest left in `/output`, walked without following links.
pub(super) struct Outputs { pub(super) files: Vec<Value>, pub(super) total: u64, keep: Option<PathBuf> }

impl Outputs {
    /// Lists regular files with their size and digest. A symlink is never
    /// followed — the guest made it and it could point anywhere on the host —
    /// and is reported as ignored, as is anything else that is not a regular
    /// file or directory.
    pub(super) fn collect(root: &Path, keep: Option<PathBuf>) -> Outputs {
        let mut outputs = Outputs { files: Vec::new(), total: 0, keep };
        outputs.walk(root, "", 0);
        outputs.files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        outputs
    }

    fn walk(&mut self, dir: &Path, prefix: &str, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten().take(MAX_OUTPUT_FILES) {
            if self.files.len() >= MAX_OUTPUT_FILES { return; }
            let name = entry.file_name().to_string_lossy().to_string();
            let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            self.visit(&entry.path(), relative, depth);
        }
    }

    fn visit(&mut self, path: &Path, relative: String, depth: usize) {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        let kind = meta.file_type();
        if kind.is_dir() {
            if depth < MAX_OUTPUT_DEPTH { self.walk(path, &relative, depth + 1); }
        } else if kind.is_file() {
            self.file(path, relative);
        } else {
            let ignored = if kind.is_symlink() { "symlink" } else { "not a regular file" };
            self.files.push(json!({"path": relative, "ignored": ignored}));
        }
    }

    fn file(&mut self, path: &Path, relative: String) {
        let Ok(bytes) = read_no_follow(path) else { return };
        self.total += bytes.len() as u64;
        if let Some(keep) = &self.keep {
            let target = keep.join(&relative);
            if let Some(parent) = target.parent() { let _ = std::fs::create_dir_all(parent); }
            let _ = std::fs::write(&target, &bytes);
        }
        self.files.push(json!({"path": relative, "bytes": bytes.len(), "sha256": sha256_hex(&bytes)}));
    }
}

fn read_no_follow(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = Vec::new();
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}
