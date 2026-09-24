//! The profile's file rules: what may be written, what inside a writable
//! mount stays protected, and what may be read.

use super::*;

/// Writes are denied first and reopened only for the granted mounts, so an
/// empty mount list leaves nothing writable but the always-writable roots.
/// After the grants come the denies a grant must not reopen: the files at a
/// mount's root a host tool trusts, the existing repository's hooks and
/// config, and the mount root itself, which stays where the policy put it.
pub(super) fn write_rules(allowed_dirs: &[String], protect: &[String]) -> String {
    let mut rules = String::from("(deny file-write*)\n");
    let writable: Vec<&str> =
        allowed_dirs.iter().filter(|dir| !dir.ends_with(":ro")).map(|dir| dir.as_str()).collect();
    for dir in &writable {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for always in always_writable() {
        rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", sandbox_literal(&always)));
    }
    for dir in &writable {
        rules.push_str(&mount_protection_rules(dir, protect));
    }
    rules
}

/// What stays closed inside one writable mount. Emitted after the grant.
///
/// A protected path can be swapped as well as written — renamed aside, a
/// replacement created, renamed back — so every ancestor of a protected path
/// up to the mount root is pinned against unlink and rename, and the mount
/// root is pinned too. A `.git` that does not exist yet is not protected: it
/// is the operator's repository this guards, not one the command creates.
pub(super) fn mount_protection_rules(dir: &str, protect: &[String]) -> String {
    let mut rules = String::new();
    let mut pinned: Vec<String> = vec![dir.to_string()];
    // `subpath` covers a file as well as a directory and all beneath it, so
    // one rule serves either, and one created later is covered too.
    for name in protect {
        rules.push_str(&format!("(deny file-write* (subpath \"{}\"))\n", sandbox_literal(&format!("{dir}/{name}"))));
        let mut parent = std::path::Path::new(name.as_str()).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            pinned.push(format!("{dir}/{}", path.display()));
            parent = path.parent();
        }
    }
    if let Some(git_dir) = repository_dir(dir) {
        rules.push_str(&format!("(deny file-write* (subpath \"{}\"))\n", sandbox_literal(&format!("{git_dir}/hooks"))));
        rules.push_str(&format!("(deny file-write* (literal \"{}\"))\n", sandbox_literal(&format!("{git_dir}/config"))));
        pinned.push(git_dir.clone());
        pinned.push(format!("{dir}/.git"));
    }
    for path in pinned {
        rules.push_str(&format!("(deny file-write-unlink (literal \"{}\"))\n", sandbox_literal(&path)));
    }
    rules
}

/// The repository directory a mount root belongs to, if it has one: `.git`
/// itself, or the directory a `.git` pointer file names (a worktree or a
/// submodule), resolved as the kernel will see it.
pub(crate) fn repository_dir(dir: &str) -> Option<String> {
    let dot_git = std::path::Path::new(dir).join(".git");
    let metadata = std::fs::symlink_metadata(&dot_git).ok()?;
    let target = if metadata.is_dir() {
        dot_git
    } else {
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let relative = pointer.trim().strip_prefix("gitdir:")?.trim();
        std::path::Path::new(dir).join(relative)
    };
    Some(std::fs::canonicalize(target).ok()?.to_string_lossy().to_string())
}

pub(super) fn read_rules(allowed_dirs: &[String], read_policy: &str) -> String {
    if read_policy == "strict" { confined_read_rules(allowed_dirs) } else { String::new() }
}

/// The paths the preset and the caller close to reads, in every mode and
/// after every grant, so a mount cannot reopen them. `file-read*` rather than
/// `file-read-data`: listing a key directory already says which hosts and
/// accounts exist.
pub(super) fn closed_read_rules(deny_read: &[String]) -> String {
    let mut rules = String::new();
    for path in deny_read {
        rules.push_str(&format!("(deny file-read* (subpath \"{}\"))\n", sandbox_literal(path)));
    }
    // The system keychain holds trust roots and nothing of the caller's, and
    // TLS needs it; the rest of that directory is other users' keychains.
    rules.push_str("(deny file-read* (subpath \"/Library/Keychains\"))\n");
    rules.push_str("(allow file-read* (literal \"/Library/Keychains/System.keychain\"))\n");
    rules
}

/// Reads are denied first and reopened for the granted mounts, the roots this
/// profile always makes writable, and the platform's own directories. Anything
/// the command needs beyond those — a language runtime's package directory,
/// say — is a mount the caller grants, not a hole this list leaves open.
pub(super) fn confined_read_rules(allowed_dirs: &[String]) -> String {
    let mut rules = String::from("(deny file-read*)\n");
    let always = always_writable();
    let granted = allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir));
    for dir in granted.chain(always.iter().map(String::as_str)).chain(PROFILE_READABLE) {
        rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", sandbox_literal(dir)));
    }
    for path in PROFILE_READABLE_LITERALS {
        rules.push_str(&format!("(allow file-read* (literal \"{}\"))\n", path));
    }
    // A tool resolving its own path walks the ancestors of every mount with
    // stat(2); metadata on those, and only metadata, stays readable.
    for dir in allowed_dirs.iter().map(|dir| dir.strip_suffix(":ro").unwrap_or(dir)) {
        let mut ancestor = std::path::Path::new(dir).parent();
        while let Some(path) = ancestor {
            if path.as_os_str().is_empty() || path == std::path::Path::new("/") { break; }
            rules.push_str(&format!(
                "(allow file-read-metadata (literal \"{}\"))\n",
                sandbox_literal(&path.to_string_lossy())
            ));
            ancestor = path.parent();
        }
    }
    rules
}

/// Everything a strict read policy leaves readable: nothing else on this host
/// can be opened, including the command porta is being asked to start. The
/// single-path literals are left out — a command is never one of them.
pub(crate) fn readable_roots(allowed_dirs: &[String]) -> Vec<String> {
    allowed_dirs
        .iter()
        .map(|dir| dir.strip_suffix(":ro").unwrap_or(dir).to_string())
        .chain(always_writable())
        .chain(PROFILE_READABLE.iter().map(|dir| dir.to_string()))
        .collect()
}
