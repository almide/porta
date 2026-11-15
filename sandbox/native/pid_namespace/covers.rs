//! What the command's mount namespace covers before the command exists: the
//! paths closed to reads, the credential sockets, and the protections inside
//! each writable mount.

use super::*;

/// What B does to one path in the command's mount namespace before the
/// command exists. The host's copy is untouched, and nothing the command does
/// can undo it: the seccomp baseline refuses mounts. Prepared before the fork,
/// so applying it allocates nothing.
pub(crate) struct Hidden {
    path: std::ffi::CString,
    how: Cover,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cover {
    /// Closed to reads: a directory under an empty tmpfs no one may enter, a
    /// file under `/dev/null`. A closed Unix socket is a file here too: a
    /// `connect` to `/dev/null` is refused.
    HideDirectory,
    HideFile,
    /// Protected inside a writable mount: bound onto itself read-only, so it
    /// can be read and not changed; and, as a mount point, not renamed or
    /// removed either.
    Freeze,
    /// Bound onto itself as it is, only to become a mount point: a mount root,
    /// or the `.git` holding protected hooks, cannot then be renamed away and
    /// replaced.
    Pin,
}

impl Hidden {
    /// The paths of `deny_read` that exist, outermost only (one inside another
    /// is covered with it, and could not be mounted on once it is), the closed
    /// Unix sockets, then the protections for each writable mount.
    pub(crate) fn prepare(closures: &crate::policy_preset::Closures, sockets: &[String], writable: &[String]) -> Vec<Hidden> {
        let exists: Vec<(&String, std::fs::Metadata)> =
            closures.deny_read.iter().filter_map(|path| std::fs::metadata(path).ok().map(|meta| (path, meta))).collect();
        let mut covers: Vec<(String, Cover)> = exists
            .iter()
            .filter(|(path, _)| !exists.iter().any(|(other, _)| other != path && path.starts_with(&format!("{other}/"))))
            .map(|(path, meta)| (path.to_string(), if meta.is_dir() { Cover::HideDirectory } else { Cover::HideFile }))
            .collect();
        covers.extend(sockets.iter().map(|socket| (socket.clone(), Cover::HideFile)));
        for mount in writable {
            covers.extend(protections(mount, closures));
        }
        covers
            .into_iter()
            .filter_map(|(path, how)| std::ffi::CString::new(path).ok().map(|path| Hidden { path, how }))
            .collect()
    }

    pub(super) fn cover(&self) -> io::Result<()> {
        let target = ptr(self.path.as_ptr());
        let locked = (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as libc::c_long;
        match self.how {
            Cover::HideDirectory => {
                let tmpfs = ptr(b"tmpfs\0".as_ptr());
                let flags = libc::MS_RDONLY as libc::c_long | locked;
                sys(libc::SYS_mount, [tmpfs, target, tmpfs, flags, ptr(b"mode=000,size=4k\0".as_ptr())]).map(drop)
            }
            Cover::HideFile => {
                sys(libc::SYS_mount, [ptr(b"/dev/null\0".as_ptr()), target, 0, libc::MS_BIND as libc::c_long, 0])?;
                // Inside a user namespace a remount must keep the flags the
                // source mount carries (/dev is nosuid, often noexec); asking
                // for read-only alone is refused.
                let flags = (libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY) as libc::c_long | locked;
                sys(libc::SYS_mount, [0, target, 0, flags, 0]).map(drop)
            }
            Cover::Freeze | Cover::Pin => {
                sys(libc::SYS_mount, [target, target, 0, (libc::MS_BIND | libc::MS_REC) as libc::c_long, 0])?;
                if self.how == Cover::Pin {
                    return Ok(());
                }
                // Read-only, keeping whatever the mount it sits on locks.
                let flags = (libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY) as libc::c_long;
                sys(libc::SYS_mount, [0, target, 0, flags, 0])
                    .or_else(|_| sys(libc::SYS_mount, [0, target, 0, flags | locked, 0]))
                    .map(drop)
            }
        }
    }
}

/// For one writable mount: each protected name that exists, frozen; the
/// repository's hooks and config, frozen; and the mount root and `.git`,
/// pinned. A name that does not exist yet is not covered on Linux — there is
/// nothing to mount on — where macOS's rules cover one created later.
pub(super) fn protections(mount: &str, closures: &crate::policy_preset::Closures) -> Vec<(String, Cover)> {
    let mut covers = vec![(mount.to_string(), Cover::Pin)];
    let exists = |path: &String| std::fs::symlink_metadata(path).map(|meta| !meta.file_type().is_symlink()).unwrap_or(false);
    if let Some(git_dir) = crate::sandbox_profile::repository_dir(mount).filter(|_| !closures.repository.is_empty()) {
        let dot_git = format!("{mount}/.git");
        if exists(&dot_git) && std::path::Path::new(&dot_git).is_dir() {
            covers.push((dot_git, Cover::Pin));
        }
        for part in &closures.repository {
            let path = format!("{git_dir}/{part}");
            if exists(&path) {
                covers.push((path, Cover::Freeze));
            }
        }
    }
    for name in crate::policy_preset::protected_in(mount, &closures.protect) {
        let path = format!("{mount}/{name}");
        if exists(&path) {
            covers.push((path, Cover::Freeze));
        }
    }
    covers
}
