#![cfg(target_os = "linux")]
//! The command's own PID and mount namespace, so `/proc` shows it and its
//! descendants and nothing else of the host.
//!
//! Landlock cannot hide another process's `/proc/<pid>`: a read grant on `/`
//! covers every pid directory, and a grant that leaves them out also takes
//! `/proc/self`, which resolves to the caller's own pid directory. A fresh
//! procfs in a PID namespace of the command's own does both at once. An
//! unprivileged process may create one only inside a user namespace of its
//! own, so all three are created together; the user namespace maps the
//! caller's uid to itself, not to root, so the command holds no capability
//! once it execs, and the seccomp baseline refuses it every further namespace.
//!
//! Three processes, all made between fork and exec:
//!
//! ```text
//! porta ── A (outside; the pid porta waits for)
//!            └── B (pid 1 inside: mounts /proc, reaps, reports)
//!                  └── C (the command: ceilings, Landlock, seccomp, exec)
//! ```
//!
//! The command cannot be pid 1 itself. The kernel drops a signal to pid 1 that
//! it has no handler for, so Ctrl-C, SIGTERM and the soft-limit SIGXCPU would
//! stop reaching a command that relies on their default action. B is pid 1
//! instead: it waits for C, collects the orphans C leaves, and when C ends
//! passes C's wait status to A and exits, which makes the kernel end
//! everything left in the namespace. A ends the way C ended, by the same exit
//! code or the same signal, so porta's supervisor sees what it saw before.
//!
//! A and B never exec, so each closes every descriptor above stderr once it has
//! forked. Otherwise they would hold the pipe std's `spawn` waits on, and
//! `spawn` would return only when the run ended. C keeps it, so a failure to
//! apply the policy or to exec still reaches `spawn` as an error.
//!
//! Some hosts refuse unprivileged user namespaces: Ubuntu from 23.10 through
//! `kernel.apparmor_restrict_unprivileged_userns`, some kernels through
//! `kernel.unprivileged_userns_clone`, and most container runtimes' seccomp
//! profiles. `available` asks once, by trying. Where the answer is no, the run
//! goes ahead without the namespace: it is defence in depth, not a rule the
//! caller asked for, and `--read-policy strict` still closes `/proc` as a
//! whole there. `porta check`, and a note in the default read mode, say so.
//!
//! Everything after the fork is a raw system call through `sys`: no
//! allocation, no lock, nothing a fork of a threaded process cannot do.

use std::io::{self, PipeReader, PipeWriter};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::OnceLock;

mod covers;
mod pid_one;
mod probe;
pub(crate) use covers::Hidden;
use pid_one::*;
pub(crate) use probe::available;

/// porta's own processes counted by RLIMIT_NPROC inside the namespace (A and
/// B). The kernel counts a user namespace's processes against the limit set
/// in it, so `--max-procs` is raised by these two and keeps meaning the
/// command's own processes.
pub(crate) const HELPER_PROCESSES: u64 = 2;

const NAMESPACES: libc::c_long = (libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWPID) as libc::c_long;
/// Under `--no-net`: a network namespace too, holding only a loopback
/// interface. The host's interfaces, its loopback services and its abstract
/// Unix sockets are not in it.
const NETWORK: libc::c_long = libc::CLONE_NEWNET as libc::c_long;

/// One system call. Every argument is a number or a pointer to memory the
/// caller owns for the length of the call.
fn sys(number: libc::c_long, args: [libc::c_long; 5]) -> io::Result<libc::c_long> {
    let result = unsafe { libc::syscall(number, args[0], args[1], args[2], args[3], args[4]) };
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

fn ptr<T>(value: *const T) -> libc::c_long {
    value as libc::c_long
}

/// A system call retried while a signal interrupts it.
fn sys_retrying(number: libc::c_long, args: [libc::c_long; 5]) -> io::Result<libc::c_long> {
    loop {
        match sys(number, args) {
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => continue,
            result => return result,
        }
    }
}

fn fork() -> io::Result<libc::pid_t> {
    // clone(SIGCHLD) is fork(2); aarch64 has no fork system call.
    sys(libc::SYS_clone, [libc::SIGCHLD as libc::c_long, 0, 0, 0, 0]).map(|pid| pid as libc::pid_t)
}

fn close(fd: RawFd) {
    let _ = sys(libc::SYS_close, [fd as libc::c_long, 0, 0, 0, 0]);
}

fn exit(code: libc::c_int) -> ! {
    let _ = sys(libc::SYS_exit_group, [code as libc::c_long, 0, 0, 0, 0]);
    unreachable!("exit_group returned")
}

fn read_all(fd: RawFd, buffer: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buffer.len() {
        let rest = &mut buffer[filled..];
        match sys_retrying(libc::SYS_read, [fd as libc::c_long, ptr(rest.as_ptr()), rest.len() as libc::c_long, 0, 0]) {
            Ok(read) if read > 0 => filled += read as usize,
            _ => break,
        }
    }
    filled
}

fn write_all(fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    let written = sys_retrying(libc::SYS_write, [fd as libc::c_long, ptr(bytes.as_ptr()), bytes.len() as libc::c_long, 0, 0])?;
    if written as usize == bytes.len() { Ok(()) } else { Err(io::Error::from_raw_os_error(libc::EIO)) }
}

fn pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as libc::c_int; 2];
    sys(libc::SYS_pipe2, [ptr(fds.as_mut_ptr()), libc::O_CLOEXEC as libc::c_long, 0, 0, 0])?;
    Ok((fds[0], fds[1]))
}

fn waitpid(pid: libc::pid_t) -> io::Result<(libc::pid_t, libc::c_int)> {
    let mut status: libc::c_int = 0;
    let waited = sys_retrying(libc::SYS_wait4, [pid as libc::c_long, ptr(&mut status), 0, 0, 0])?;
    Ok((waited as libc::pid_t, status))
}

/// One `/proc/self/{uid,gid}_map` line, formatted before the fork so the child
/// only has to write it.
#[derive(Clone, Copy)]
struct MapLine {
    bytes: [u8; 40],
    len: usize,
}

impl MapLine {
    fn identity(id: libc::c_long) -> Self {
        let text = format!("{id} {id} 1\n");
        let mut bytes = [0u8; 40];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        MapLine { bytes, len: text.len() }
    }
}

/// Everything the child needs to enter the namespaces. Plain data, so the
/// `pre_exec` closure that carries it copies and allocates nothing.
#[derive(Clone, Copy)]
pub(crate) struct Isolation {
    uid_map: MapLine,
    gid_map: MapLine,
    network: bool,
    /// Under a memory ceiling: A writes its pid on `ready` and waits for a
    /// byte on `go` before it forks, so porta can place it in the ceiling's
    /// cgroup while nothing of the command exists yet. A also holds copies of
    /// porta's ends from the fork and closes them first: while it held the
    /// write end of `go` it would never see the end of file that means "not
    /// placed".
    gate: Option<[RawFd; 4]>,
}

/// porta's side of the gate: its own ends, and the child's ends, which porta
/// holds until the child has them and then drops.
pub(crate) struct Gate {
    pub(crate) ready: PipeReader,
    pub(crate) go: PipeWriter,
    pub(crate) child_ends: (PipeWriter, PipeReader),
}

impl Isolation {
    /// The isolation for one run, the gate when `gated`, and a network
    /// namespace of its own when `network`.
    pub(crate) fn prepare(gated: bool, network: bool) -> io::Result<(Isolation, Option<Gate>)> {
        let uid_map = MapLine::identity(sys(libc::SYS_getuid, [0; 5])?);
        let gid_map = MapLine::identity(sys(libc::SYS_getgid, [0; 5])?);
        if !gated {
            return Ok((Isolation { uid_map, gid_map, network, gate: None }, None));
        }
        let (ready, ready_child) = io::pipe()?;
        let (go_child, go) = io::pipe()?;
        let fds = [ready_child.as_raw_fd(), go_child.as_raw_fd(), ready.as_raw_fd(), go.as_raw_fd()];
        let gate = Gate { ready, go, child_ends: (ready_child, go_child) };
        Ok((Isolation { uid_map, gid_map, network, gate: Some(fds) }, Some(gate)))
    }

    /// The first `pre_exec` step. Returns only in C; A and B stay here until
    /// the run ends and exit from here. `hidden` is what B covers before the
    /// command exists (see `Hidden`).
    pub(crate) fn enter(self, hidden: &[Hidden]) -> io::Result<()> {
        sys(libc::SYS_unshare, [NAMESPACES | if self.network { NETWORK } else { 0 }, 0, 0, 0, 0])?;
        self.map_and_privatise()?;
        if self.network {
            loopback_up()?;
        }
        if let Some(gate) = self.gate {
            pass_gate(gate)?;
        }
        let (status_read, status_write) = pipe()?;
        let init = fork()?;
        if init != 0 {
            close(status_write);
            close_all_above_stderr_except(status_read);
            relay(init, status_read)
        }
        close(status_read);
        mount_proc()?;
        for path in hidden {
            path.cover()?;
        }
        let command = fork()?;
        if command != 0 {
            close_all_above_stderr_except(status_write);
            reap_until(command, status_write)
        }
        close(status_write);
        Ok(())
    }

    /// The uid and gid mapped to themselves, and mount events kept from
    /// reaching the host. Right after the unshare.
    fn map_and_privatise(&self) -> io::Result<()> {
        // An unprivileged process may write its gid map only once it has
        // given up setgroups(2).
        write_file(b"/proc/self/setgroups\0", b"deny")?;
        write_file(b"/proc/self/uid_map\0", &self.uid_map.bytes[..self.uid_map.len])?;
        write_file(b"/proc/self/gid_map\0", &self.gid_map.bytes[..self.gid_map.len])?;
        let flags = (libc::MS_REC | libc::MS_PRIVATE) as libc::c_long;
        sys(libc::SYS_mount, [0, ptr(b"/\0".as_ptr()), 0, flags, 0]).map(drop)
    }
}

fn write_file(path: &[u8], contents: &[u8]) -> io::Result<()> {
    let flags = (libc::O_WRONLY | libc::O_CLOEXEC) as libc::c_long;
    let fd = sys(libc::SYS_openat, [libc::AT_FDCWD as libc::c_long, ptr(path.as_ptr()), flags, 0, 0])? as RawFd;
    let written = write_all(fd, contents);
    close(fd);
    written
}

/// The new network namespace's loopback interface, which starts down. A
/// command that talks to itself over 127.0.0.1 still can.
fn loopback_up() -> io::Result<()> {
    let socket = sys(libc::SYS_socket, [libc::AF_INET as libc::c_long, (libc::SOCK_DGRAM | libc::SOCK_CLOEXEC) as libc::c_long, 0, 0, 0])? as RawFd;
    // struct ifreq: the interface name, then the flags as a short.
    let mut request = [0u8; 40];
    request[..2].copy_from_slice(b"lo");
    request[16..18].copy_from_slice(&((libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short).to_ne_bytes());
    let raised = sys(libc::SYS_ioctl, [socket as libc::c_long, libc::SIOCSIFFLAGS as libc::c_long, ptr(request.as_ptr()), 0, 0]);
    close(socket);
    raised.map(drop)
}

/// A procfs for the new PID namespace over `/proc`. Runs in B, the first
/// process inside it.
fn mount_proc() -> io::Result<()> {
    let flags = (libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC) as libc::c_long;
    let proc = ptr(b"proc\0".as_ptr());
    sys(libc::SYS_mount, [proc, ptr(b"/proc\0".as_ptr()), proc, flags, 0]).map(drop)
}

/// Tells porta A's pid and waits for its go-ahead. End of file instead of the
/// byte means porta could not place the run under its ceiling.
fn pass_gate([ready, go, porta_ready, porta_go]: [RawFd; 4]) -> io::Result<()> {
    close(porta_ready);
    close(porta_go);
    let pid = (sys(libc::SYS_getpid, [0; 5])? as libc::pid_t).to_ne_bytes();
    let mut byte = [0u8; 1];
    let passed = write_all(ready, &pid).is_ok() && read_all(go, &mut byte) == 1;
    close(ready);
    close(go);
    if passed { Ok(()) } else { Err(io::Error::from_raw_os_error(libc::ECANCELED)) }
}

/// Every descriptor from 3 up but `keep`: std's exec-status pipe among them,
/// which a process that never execs must not hold.
fn close_all_above_stderr_except(keep: RawFd) {
    for (low, high) in [(3, keep as libc::c_long - 1), (keep as libc::c_long + 1, libc::c_uint::MAX as libc::c_long)] {
        if low <= high {
            let _ = sys(libc::SYS_close_range, [low, high, 0, 0, 0]);
        }
    }
}
