//! B, the namespace's pid 1: it reaps, relays the group signals to the
//! command, and reports how the command ended.

use super::*;

/// B, pid 1 inside: reaps until the command ends, then hands its wait status
/// to A. Exiting ends everything else left in the namespace.
pub(super) fn reap_until(command: libc::pid_t, status_write: RawFd) -> ! {
    loop {
        match waitpid(-1) {
            Ok((pid, status)) if pid == command => {
                let _ = write_all(status_write, &status.to_ne_bytes());
                exit(0)
            }
            Ok(_) => continue,
            Err(_) => exit(125),
        }
    }
}

pub(super) fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) {
    // The kernel's struct sigaction: handler, flags, restorer, mask.
    let action: [libc::c_ulong; 4] = [handler as libc::c_ulong, 0, 0, 0];
    let _ = sys(libc::SYS_rt_sigaction, [signal as libc::c_long, ptr(action.as_ptr()), 0, 8, 0]);
}

pub(super) const GROUP_SIGNALS: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

/// A, outside: waits for the command's wait status from B, or for B's own if B
/// died without sending it, and ends the same way.
pub(super) fn relay(init: libc::pid_t, status_read: RawFd) -> ! {
    // Signals meant for the command reach A too, as a member of its process
    // group. A outlives them to report how the command ended; a SIGKILL still
    // ends it, and B and C with it.
    for signal in GROUP_SIGNALS {
        set_disposition(signal, libc::SIG_IGN);
    }
    let mut bytes = [0u8; 4];
    let from_init = read_all(status_read, &mut bytes) == bytes.len();
    let init_status = waitpid(init).map(|(_, status)| status).unwrap_or(0);
    end_as(if from_init { libc::c_int::from_ne_bytes(bytes) } else { init_status })
}

/// Exits with `status`'s code, or dies of its signal without a core dump.
pub(super) fn end_as(status: libc::c_int) -> ! {
    if libc::WIFEXITED(status) {
        exit(libc::WEXITSTATUS(status))
    }
    if !libc::WIFSIGNALED(status) {
        exit(125)
    }
    let signal = libc::WTERMSIG(status);
    let no_core = [0 as libc::c_ulong; 2];
    let _ = sys(libc::SYS_prlimit64, [0, libc::RLIMIT_CORE as libc::c_long, ptr(no_core.as_ptr()), 0, 0]);
    set_disposition(signal, libc::SIG_DFL);
    let pid = sys(libc::SYS_getpid, [0; 5]).unwrap_or(0);
    let _ = sys(libc::SYS_kill, [pid, signal as libc::c_long, 0, 0, 0]);
    exit(128 + signal)
}
