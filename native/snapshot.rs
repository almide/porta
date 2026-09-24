//! Putting a run's writes back.
//!
//! Under `--snapshot`, porta copies every writable mount before the command
//! starts — a clone on APFS, so it costs next to nothing there; a reflink
//! where the Linux filesystem has them, a copy where it does not — and says
//! afterwards what the run changed. `porta rollback` shows the same changes
//! against the latest snapshot and, with `--yes`, puts each mount back as it
//! was: files the run added are removed, and files it changed or removed are
//! restored.
//!
//! The snapshots live under `~/.porta/snapshots`, outside every mount, so the
//! command they were taken for cannot reach them to rewrite its own undo.

use std::path::{Path, PathBuf};

/// How many snapshots are kept; the oldest go when a new one is taken.
const KEPT: usize = 10;

fn root() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "--snapshot keeps its copies under ~/.porta, and HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".porta").join("snapshots"))
}

/// Copies each of `mounts` (absolute, writable) and returns the snapshot's
/// id. A mount holding the snapshot directory is refused: the command could
/// then rewrite its own undo.
pub(crate) fn take(mounts: &[String], command: &str) -> Result<String, String> {
    let root = root()?;
    if let Some(mount) = mounts.iter().find(|mount| root.starts_with(mount.as_str())) {
        return Err(format!("--snapshot keeps its copies in {}, inside the mount {mount}; the command could rewrite them", root.display()));
    }
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0);
    let id = format!("{seconds}-{}", std::process::id());
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).map_err(|error| format!("cannot create the snapshot in {}: {error}", dir.display()))?;
    for (index, mount) in mounts.iter().enumerate() {
        clone_tree(Path::new(mount), &dir.join(index.to_string())).map_err(|error| format!("cannot snapshot {mount}: {error}"))?;
    }
    let manifest = serde_json::json!({ "mounts": mounts, "command": command });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).map_err(|error| format!("cannot write the snapshot: {error}"))?;
    prune(&root);
    Ok(id)
}

/// The oldest snapshots past [`KEPT`], removed. Ids begin with the second
/// they were taken, so their order is their age.
fn prune(root: &Path) {
    let mut ids = ids(root);
    while ids.len() > KEPT {
        let _ = std::fs::remove_dir_all(root.join(ids.remove(0)));
    }
}

fn ids(root: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(root)
        .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    ids.sort_by_key(|id| id.split('-').next().and_then(|seconds| seconds.parse::<u64>().ok()).unwrap_or(0));
    ids
}

/// `source` copied to `target`, which must not exist: one clone of the whole
/// tree on macOS when the filesystem allows it, otherwise file by file.
fn clone_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    if clone_whole(source, target) {
        return Ok(());
    }
    copy_tree(source, target)
}

#[cfg(target_os = "macos")]
fn clone_whole(source: &Path, target: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(from), Ok(to)) = (std::ffi::CString::new(source.as_os_str().as_bytes()), std::ffi::CString::new(target.as_os_str().as_bytes())) else {
        return false;
    };
    // clonefile(2) copies a directory tree as copy-on-write clones, in one
    // call; CLONE_NOFOLLOW (<sys/clonefile.h>) clones a symlink as itself.
    const CLONE_NOFOLLOW: u32 = 0x0001;
    unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), CLONE_NOFOLLOW) == 0 }
}

/// A tree copied file by file: directories with their modes, symlinks as
/// links, regular files through `std::fs::copy` (which reflinks where the
/// Linux filesystem can). Anything else — a socket, a device — is skipped.
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir(target)?;
    std::fs::set_permissions(target, std::fs::metadata(source)?.permissions())?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// What a run changed in one mount, relative to it.
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Changes {
    pub(crate) added: Vec<String>,
    pub(crate) changed: Vec<String>,
    pub(crate) removed: Vec<String>,
}

impl Changes {
    fn count(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }
}

/// The entries under `dir`, relative to it, each once, directories first.
fn entries(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.sort();
    found
}

fn walk(base: &Path, dir: &Path, found: &mut Vec<String>) {
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    for entry in listing.flatten() {
        let path = entry.path();
        found.push(path.strip_prefix(base).map(|relative| relative.to_string_lossy().to_string()).unwrap_or_default());
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            walk(base, &path, found);
        }
    }
}

/// Whether two entries differ: in kind, in a link's target, or in a file's
/// bytes. Directories are the same when both are directories; their contents
/// are compared entry by entry.
fn differs(before: &Path, now: &Path) -> bool {
    let (Ok(old), Ok(new)) = (std::fs::symlink_metadata(before), std::fs::symlink_metadata(now)) else { return true };
    if old.file_type() != new.file_type() {
        return true;
    }
    if old.file_type().is_symlink() {
        return std::fs::read_link(before).ok() != std::fs::read_link(now).ok();
    }
    if old.is_file() {
        return old.len() != new.len() || old.permissions() != new.permissions() || std::fs::read(before).ok() != std::fs::read(now).ok();
    }
    false
}

/// `mount` against its copy in `snapshot`.
pub(crate) fn changes(snapshot: &Path, mount: &Path) -> Changes {
    let before = entries(snapshot);
    let now = entries(mount);
    let mut changes = Changes::default();
    for entry in &now {
        if before.binary_search(entry).is_err() {
            changes.added.push(entry.clone());
        } else if differs(&snapshot.join(entry), &mount.join(entry)) {
            changes.changed.push(entry.clone());
        }
    }
    changes.removed = before.into_iter().filter(|entry| now.binary_search(entry).is_err()).collect();
    changes
}

/// Puts `mount` back as `snapshot` holds it: what was added goes, deepest
/// first; what was changed or removed comes back from the copy.
fn restore(snapshot: &Path, mount: &Path, changes: &Changes) -> std::io::Result<()> {
    for entry in changes.added.iter().rev() {
        let path = mount.join(entry);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&path)?,
            Ok(_) => std::fs::remove_file(&path)?,
            Err(_) => {}
        }
    }
    for entry in changes.changed.iter().chain(&changes.removed) {
        let (from, to) = (snapshot.join(entry), mount.join(entry));
        if std::fs::symlink_metadata(&to).is_ok_and(|meta| meta.is_dir()) && !from.is_dir() {
            std::fs::remove_dir_all(&to)?;
        } else if std::fs::symlink_metadata(&to).is_ok_and(|meta| !meta.is_dir()) {
            std::fs::remove_file(&to)?;
        }
        if std::fs::symlink_metadata(&to).is_err() {
            clone_tree_entry(&from, &to)?;
        }
    }
    Ok(())
}

/// One entry of a snapshot put back at `to`: a directory as a directory (its
/// contents come back entry by entry), a link as a link, a file as a copy.
fn clone_tree_entry(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        std::fs::set_permissions(to, meta.permissions())
    } else if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    } else {
        std::fs::copy(from, to).map(drop)
    }
}

/// The snapshot `id`, or the latest one, with the mounts it copied.
fn open(id: &str) -> Result<(String, PathBuf, Vec<String>), String> {
    let root = root()?;
    let id = if id.is_empty() { ids(&root).pop().ok_or("there is no snapshot; run with --snapshot first")? } else { id.to_string() };
    let dir = root.join(&id);
    let manifest: serde_json::Value = std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or(format!("no snapshot {id} in {}", root.display()))?;
    let mounts = manifest["mounts"].as_array().map(|mounts| mounts.iter().filter_map(|mount| mount.as_str().map(str::to_string)).collect()).unwrap_or_default();
    Ok((id, dir, mounts))
}

/// What the run changed, in words, one mount after another.
pub(crate) fn describe(id: &str, dir: &Path, mounts: &[String]) -> String {
    let mut text = String::new();
    for (index, mount) in mounts.iter().enumerate() {
        let changes = changes(&dir.join(index.to_string()), Path::new(mount));
        text.push_str(&format!("[porta] snapshot {id}: {} change(s) in {mount}\n", changes.count()));
        for (mark, list) in [("+", &changes.added), ("~", &changes.changed), ("-", &changes.removed)] {
            for entry in list.iter().take(20) {
                text.push_str(&format!("  {mark} {entry}\n"));
            }
            if list.len() > 20 {
                text.push_str(&format!("  {mark} … and {} more\n", list.len() - 20));
            }
        }
    }
    text
}

/// What the run that snapshot `id` was taken for changed, and how to undo it.
pub(crate) fn after_run(id: &str, mounts: &[String]) -> String {
    let Ok(root) = root() else { return String::new() };
    format!("{}[porta] `porta rollback {id}` shows these again; add --yes to put them back\n", describe(id, &root.join(id), mounts))
}

/// `porta rollback [id] [--yes]`: the changes since snapshot `id` (the latest
/// when empty) and, when `apply`, each mount put back.
pub fn wt_rollback(id: impl AsRef<str>, apply: bool) -> String {
    rollback(id.as_ref(), apply).trim_end().to_string()
}

fn rollback(id: &str, apply: bool) -> String {
    let (id, dir, mounts) = match open(id) {
        Ok(found) => found,
        Err(reason) => return format!("Error: {reason}\n"),
    };
    let mut text = describe(&id, &dir, &mounts);
    if !apply {
        text.push_str(&format!("run `porta rollback {id} --yes` to put these back\n"));
        return text;
    }
    for (index, mount) in mounts.iter().enumerate() {
        let snapshot = dir.join(index.to_string());
        if let Err(error) = restore(&snapshot, Path::new(mount), &changes(&snapshot, Path::new(mount))) {
            return format!("{text}Error: restoring {mount} stopped part way: {error}\n");
        }
    }
    text.push_str(&format!("[porta] put back as snapshot {id} held them\n"));
    text
}

#[cfg(test)]
mod tests;
