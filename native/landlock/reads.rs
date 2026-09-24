//! Read grants: the platform's own directories under a strict policy, and
//! everything but the closed paths under an open one.

use super::*;

/// Listing a directory, and nothing beneath it.
pub(super) const ACCESS_FS_READ_DIR: u64 = 1 << 3;

/// Reads everywhere except beneath `closed`. Landlock is an allow-list and a
/// grant covers everything under it, so the walk descends only along closed
/// paths: a subtree holding none is granted whole, a directory holding one is
/// granted listing only and its entries visited, a closed path is skipped. A
/// symlink is never granted: the kernel checks the path it resolves to, which
/// has its own place in the walk. An entry that cannot be opened, or appears
/// after the walk in a directory on a closed path, stays closed.
pub(super) fn open_except(ruleset: &Ruleset, dir: &std::path::Path, closed: &[String]) -> Result<(), String> {
    let here = dir.to_string_lossy();
    let prefix = if here == "/" { "/".to_string() } else { format!("{here}/") };
    if closed.iter().any(|path| *path == here) {
        return Ok(());
    }
    if !closed.iter().any(|path| path.starts_with(&prefix)) {
        let _ = allow_path(ruleset, &here, READ_RIGHTS_ABI1);
        return Ok(());
    }
    allow_path(ruleset, &here, ACCESS_FS_READ_DIR)?;
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.flatten() {
        if entry.file_type().map(|kind| kind.is_symlink()).unwrap_or(true) {
            continue;
        }
        open_except(ruleset, &entry.path(), closed)?;
    }
    Ok(())
}

/// Reading is handled all-or-nothing: with reads restricted, a path no rule
/// names is closed, so the platform's own directories have to be named too.
pub(super) fn allow_reads(ruleset: &Ruleset, policy: &Policy) -> Result<(), String> {
    let present = |path: &&String| std::path::Path::new(path.as_str()).exists();
    let system = policy.system_dirs.iter().chain(&policy.system_files).filter(present);
    for path in policy.readable_dirs.iter().chain(system) {
        allow_path(ruleset, path, READ_RIGHTS_ABI1)?;
    }
    Ok(())
}
