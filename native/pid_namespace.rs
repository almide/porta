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
    pub(crate) fn prepare(deny_read: &[String], sockets: &[String], writable: &[String], protect: &[String]) -> Vec<Hidden> {
        let exists: Vec<(&String, std::fs::Metadata)> =
            deny_read.iter().filter_map(|path| std::fs::metadata(path).ok().map(|meta| (path, meta))).collect();
        let mut covers: Vec<(String, Cover)> = exists
            .iter()
            .filter(|(path, _)| !exists.iter().any(|(other, _)| other != path && path.starts_with(&format!("{other}/"))))
            .map(|(path, meta)| (path.to_string(), if meta.is_dir() { Cover::HideDirectory } else { Cover::HideFile }))
            .collect();
        covers.extend(sockets.iter().map(|socket| (socket.clone(), Cover::HideFile)));
        for mount in writable {
            covers.extend(protections(mount, protect));
        }
        covers
            .into_iter()
            .filter_map(|(path, how)| std::ffi::CString::new(path).ok().map(|path| Hidden { path, how }))
            .collect()
    }

    fn cover(&self) -> io::Result<()> {
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
fn protections(mount: &str, protect: &[String]) -> Vec<(String, Cover)> {
    let mut covers = vec![(mount.to_string(), Cover::Pin)];
    let exists = |path: &String| std::fs::symlink_metadata(path).map(|meta| !meta.file_type().is_symlink()).unwrap_or(false);
    if let Some(git_dir) = crate::sandbox_profile::repository_dir(mount) {
        let dot_git = format!("{mount}/.git");
        if exists(&dot_git) && std::path::Path::new(&dot_git).is_dir() {
            covers.push((dot_git, Cover::Pin));
        }
        for part in ["hooks", "config"] {
            let path = format!("{git_dir}/{part}");
            if exists(&path) {
                covers.push((path, Cover::Freeze));
            }
        }
    }
    for name in protect {
        let path = format!("{mount}/{name}");
        if exists(&path) {
            covers.push((path, Cover::Freeze));
        }
    }
    covers
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

/// B, pid 1 inside: reaps until the command ends, then hands its wait status
/// to A. Exiting ends everything else left in the namespace.
fn reap_until(command: libc::pid_t, status_write: RawFd) -> ! {
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

fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) {
    // The kernel's struct sigaction: handler, flags, restorer, mask.
    let action: [libc::c_ulong; 4] = [handler as libc::c_ulong, 0, 0, 0];
    let _ = sys(libc::SYS_rt_sigaction, [signal as libc::c_long, ptr(action.as_ptr()), 0, 8, 0]);
}

const GROUP_SIGNALS: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

/// A, outside: waits for the command's wait status from B, or for B's own if B
/// died without sending it, and ends the same way.
fn relay(init: libc::pid_t, status_read: RawFd) -> ! {
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
fn end_as(status: libc::c_int) -> ! {
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

/// Whether this host lets an unprivileged process have the namespaces, asked
/// once by trying: a child enters them and its child mounts a procfs. `Err`
/// says why not.
pub(crate) fn available() -> Result<(), &'static str> {
    static ANSWER: OnceLock<Result<(), &'static str>> = OnceLock::new();
    *ANSWER.get_or_init(probe)
}

const PROBE_REFUSALS: [&str; 3] = [
    "this host refuses an unprivileged user namespace (a container's seccomp profile, user.max_user_namespaces=0, or kernel.unprivileged_userns_clone=0)",
    "this host gives an unprivileged user namespace no rights to mount in (on Ubuntu, kernel.apparmor_restrict_unprivileged_userns=1: scripts/apparmor-userns.sh grants porta alone)",
    "a fresh /proc cannot be mounted here (the host's /proc has mounts over parts of it, as in most containers)",
];

fn probe() -> Result<(), &'static str> {
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
fn probe_child(isolation: &Isolation) -> libc::c_int {
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
