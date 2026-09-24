//! Whether this host gives an unprivileged process the namespaces, asked by
//! trying once.

use super::*;

/// Whether this host lets an unprivileged process have the namespaces, asked
/// once by trying: a child enters them and its child mounts a procfs. `Err`
/// says why not.
pub(crate) fn available() -> Result<(), &'static str> {
    static ANSWER: OnceLock<Result<(), &'static str>> = OnceLock::new();
    *ANSWER.get_or_init(probe)
}

pub(super) const PROBE_REFUSALS: [&str; 3] = [
    "this host refuses an unprivileged user namespace (a container's seccomp profile, user.max_user_namespaces=0, or kernel.unprivileged_userns_clone=0)",
    "this host gives an unprivileged user namespace no rights to mount in (on Ubuntu, kernel.apparmor_restrict_unprivileged_userns=1: scripts/apparmor-userns.sh grants porta alone)",
    "a fresh /proc cannot be mounted here (the host's /proc has mounts over parts of it, as in most containers)",
];

pub(super) fn probe() -> Result<(), &'static str> {
    let Ok((isolation, _)) = Isolation::prepare(false, true) else { return Err(PROBE_REFUSALS[0]) };
    match fork() {
        Ok(0) => exit(probe_child(&isolation)),
        Ok(child) => match waitpid(child) {
            Ok((_, 0)) => Ok(()),
            Ok((_, status)) if libc::WIFEXITED(status) && (1..=3).contains(&libc::WEXITSTATUS(status)) => {
                Err(PROBE_REFUSALS[libc::WEXITSTATUS(status) as usize - 1])
            }
            _ => Err(PROBE_REFUSALS[0]),
        },
        Err(_) => Err(PROBE_REFUSALS[0]),
    }
}

/// The probe's child: 0 when everything worked, otherwise the stage that
/// failed. Raw system calls only; it runs after a fork of a threaded process.
pub(super) fn probe_child(isolation: &Isolation) -> libc::c_int {
    // The network namespace is asked for too, so an answer of yes holds for
    // --no-net as well.
    if sys(libc::SYS_unshare, [NAMESPACES | NETWORK, 0, 0, 0, 0]).is_err() {
        return 1;
    }
    if isolation.map_and_privatise().is_err() || loopback_up().is_err() {
        return 2;
    }
    match fork() {
        Ok(0) => exit(if mount_proc().is_ok() { 0 } else { 3 }),
        Ok(grandchild) => match waitpid(grandchild) {
            Ok((_, status)) if libc::WIFEXITED(status) => libc::WEXITSTATUS(status),
            _ => 3,
        },
        Err(_) => 3,
    }
}
