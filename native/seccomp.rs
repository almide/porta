//! The part of a policy Landlock cannot express.
//!
//! Landlock sees files and TCP ports. It does not see the syscalls that reach
//! around a file policy — a memory file handed to `execveat`, a debugger
//! attaching to a sibling, a ring that opens sockets without `socket(2)` — nor
//! the egress channels a TCP rule never names: UDP, raw and packet sockets,
//! netlink families, MPTCP, which the kernel routes past the TCP hooks. A
//! seccomp filter closes those at the one syscall each of them needs.
//!
//! Two programs are built, in the parent, before any fork:
//!
//! - **Baseline**, installed in every mode. It denies the syscalls above and
//!   refuses `io_uring`, `clone3` (so a thread library falls back to `clone`,
//!   whose flags this filter can read) and any `clone` asking for a new
//!   namespace. Everything else passes untouched.
//! - **Proxy-only**, installed when the run claims the loopback proxy is its
//!   only egress. On top of the baseline, `socket(2)` is refused unless it asks
//!   for `AF_INET` with `SOCK_STREAM`; Landlock still decides *which* TCP port
//!   that socket may reach, so the two together are the invariant: one
//!   loopback endpoint, no UDP, no Unix sockets.
//!
//! Every refusal is an errno a program already knows how to handle — `EPERM`,
//! `ENOSYS`, `EAFNOSUPPORT` — except one: a process whose syscall numbers come
//! from another architecture's table is killed, because for it any errno would
//! be a lie about which call was refused.
//!
//! `socketpair(2)` is deliberately untouched. It makes an anonymous pair both
//! of whose ends the process already holds; it reaches nothing, and runtimes
//! use it for their own plumbing. `memfd_create` is untouched for the same
//! reason — language runtimes use it for their own heaps — and the exec of a
//! memory file is what is refused instead.
#![cfg(target_os = "linux")]

use std::sync::OnceLock;

mod assembler;
use assembler::*;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e; // AUDIT_ARCH_X86_64
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7; // AUDIT_ARCH_AARCH64

/// Added after the syscall tables were unified, so the same number on every
/// architecture this builds for.
const SYS_IO_URING_SETUP: u32 = 425;
const SYS_IO_URING_ENTER: u32 = 426;
const SYS_IO_URING_REGISTER: u32 = 427;
const SYS_OPEN_TREE: u32 = 428;
const SYS_MOVE_MOUNT: u32 = 429;
const SYS_FSOPEN: u32 = 430;
const SYS_FSCONFIG: u32 = 431;
const SYS_FSMOUNT: u32 = 432;
const SYS_FSPICK: u32 = 433;
const SYS_CLONE3: u32 = 435;
const SYS_PIDFD_GETFD: u32 = 438;
const SYS_MOUNT_SETATTR: u32 = 442;

/// Syscalls refused with `EPERM` in every mode: attaching to or reading
/// another process, mounting, entering or creating namespaces, loading kernel
/// code, machine-wide state, and the fileless-exec and bypass primitives.
const DENIED: &[u32] = &[
    libc::SYS_ptrace as u32,
    libc::SYS_process_vm_readv as u32,
    libc::SYS_process_vm_writev as u32,
    SYS_PIDFD_GETFD,
    libc::SYS_userfaultfd as u32,
    libc::SYS_keyctl as u32,
    libc::SYS_add_key as u32,
    libc::SYS_request_key as u32,
    libc::SYS_bpf as u32,
    libc::SYS_perf_event_open as u32,
    libc::SYS_mount as u32,
    libc::SYS_umount2 as u32,
    libc::SYS_pivot_root as u32,
    SYS_OPEN_TREE,
    SYS_MOVE_MOUNT,
    SYS_FSOPEN,
    SYS_FSCONFIG,
    SYS_FSMOUNT,
    SYS_FSPICK,
    SYS_MOUNT_SETATTR,
    libc::SYS_unshare as u32,
    libc::SYS_setns as u32,
    libc::SYS_open_by_handle_at as u32,
    libc::SYS_personality as u32,
    libc::SYS_kexec_load as u32,
    libc::SYS_kexec_file_load as u32,
    libc::SYS_init_module as u32,
    libc::SYS_finit_module as u32,
    libc::SYS_delete_module as u32,
    libc::SYS_reboot as u32,
    libc::SYS_swapon as u32,
    libc::SYS_swapoff as u32,
    libc::SYS_acct as u32,
    libc::SYS_quotactl as u32,
    libc::SYS_sethostname as u32,
    libc::SYS_setdomainname as u32,
    libc::SYS_lookup_dcookie as u32,
    libc::SYS_nfsservctl as u32,
];

/// Port I/O exists on x86 alone.
#[cfg(target_arch = "x86_64")]
const DENIED_ARCH: &[u32] = &[libc::SYS_ioperm as u32, libc::SYS_iopl as u32];
#[cfg(not(target_arch = "x86_64"))]
const DENIED_ARCH: &[u32] = &[];

/// Syscalls answered with `ENOSYS`, as a kernel that lacks them would: the
/// ring, and `clone3`, whose flags live in memory this filter cannot read.
const NO_SYSCALL: &[u32] = &[SYS_IO_URING_SETUP, SYS_IO_URING_ENTER, SYS_IO_URING_REGISTER, SYS_CLONE3];

/// `clone(2)` flags that create a namespace. Any of them set is refused.
const CLONE_NEWNS_ANY: u32 = 0x0002_0000 // CLONE_NEWNS
    | 0x0200_0000 // CLONE_NEWCGROUP
    | 0x0400_0000 // CLONE_NEWUTS
    | 0x0800_0000 // CLONE_NEWIPC
    | 0x1000_0000 // CLONE_NEWUSER
    | 0x2000_0000 // CLONE_NEWPID
    | 0x4000_0000 // CLONE_NEWNET
    | 0x0000_0080; // CLONE_NEWTIME

/// `ioctl` requests that push input into a terminal another process reads.
const TIOCSTI: u32 = 0x5412;
const TIOCLINUX: u32 = 0x541c;

/// `execveat` with this flag runs the file behind a descriptor, path unseen.
const AT_EMPTY_PATH: u32 = 0x1000;

const AF_INET: u32 = libc::AF_INET as u32;
const AF_INET6: u32 = libc::AF_INET6 as u32;
const IPPROTO_TCP: u32 = libc::IPPROTO_TCP as u32;
const AF_NETLINK: u32 = libc::AF_NETLINK as u32;
const AF_PACKET: u32 = libc::AF_PACKET as u32;
const AF_BLUETOOTH: u32 = libc::AF_BLUETOOTH as u32;
const AF_VSOCK: u32 = libc::AF_VSOCK as u32;
const SOCK_RAW: u32 = libc::SOCK_RAW as u32;
const SOCK_STREAM: u32 = libc::SOCK_STREAM as u32;
/// `type` carries `SOCK_NONBLOCK` and `SOCK_CLOEXEC` in its high bits.
const SOCK_TYPE_MASK: u32 = 0xf;
const NETLINK_ROUTE: u32 = 0;
/// Multipath TCP is routed past Landlock's TCP hooks, so a port rule does not
/// see it; a socket asking for it is refused.
const IPPROTO_MPTCP: u32 = 262;

/// Which egress channels this run is allowed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Egress {
    /// No network rule: the baseline alone.
    Open,
    /// `--allow-net` names TCP ports. Landlock holds the port rule; the
    /// baseline closes the families a TCP rule cannot see, and an internet
    /// socket must be TCP, which closes UDP, SCTP and ICMP echo.
    TcpPorts,
    /// The loopback proxy is the only permitted egress.
    ProxyOnly,
}

/// The program for one egress mode.
fn assemble(egress: Egress) -> Vec<Instruction> {
    let mut asm = Assembler::new();
    // arch must be the one these syscall numbers belong to
    asm.load(OFFSET_ARCH);
    asm.if_equal(AUDIT_ARCH, Target::Next, Target::Verdict(Verdict::Kill));
    asm.load(OFFSET_NR);
    asm.refuse_each(DENIED, Verdict::Eperm);
    asm.refuse_each(DENIED_ARCH, Verdict::Eperm);
    asm.refuse_each(NO_SYSCALL, Verdict::Enosys);
    // clone asking for any new namespace
    asm.for_syscall(libc::SYS_clone as u32, |asm| {
        asm.load(arg_low(0));
        asm.if_any(CLONE_NEWNS_ANY, Target::Verdict(Verdict::Eperm), Target::Verdict(Verdict::Allow));
    });
    // ioctl pushing input into a terminal
    asm.for_syscall(libc::SYS_ioctl as u32, |asm| {
        asm.load(arg_low(1));
        asm.if_equal(TIOCSTI, Target::Verdict(Verdict::Eperm), Target::Next);
        asm.if_equal(TIOCLINUX, Target::Verdict(Verdict::Eperm), Target::Verdict(Verdict::Allow));
    });
    // execveat of a descriptor with no path: a memory file, most likely
    asm.for_syscall(libc::SYS_execveat as u32, |asm| {
        asm.load(arg_low(4));
        asm.if_any(AT_EMPTY_PATH, Target::Verdict(Verdict::Eperm), Target::Verdict(Verdict::Allow));
    });
    asm.for_syscall(libc::SYS_socket as u32, |asm| socket_rules(asm, egress));
    asm.jump(Target::Verdict(Verdict::Allow));
    asm.finish()
}

/// What `socket(2)` may ask for. The accumulator is free on entry.
fn socket_rules(asm: &mut Assembler, egress: Egress) {
    let refuse = Target::Verdict(Verdict::Eafnosupport);
    // protocol: multipath TCP is never a TCP rule's business
    asm.load(arg_low(2));
    asm.if_equal(IPPROTO_MPTCP, Target::Verdict(Verdict::Eprotonosupport), Target::Next);
    // type: raw sockets never; the high word must be empty
    asm.load(arg_high(1));
    asm.if_equal(0, Target::Next, refuse);
    asm.load(arg_low(1));
    asm.mask(SOCK_TYPE_MASK);
    asm.if_equal(SOCK_RAW, refuse, Target::Next);
    if egress == Egress::ProxyOnly {
        // only a plain TCP socket, and only over IPv4, may leave
        asm.if_equal(SOCK_STREAM, Target::Next, refuse);
        asm.load(arg_high(0));
        asm.if_equal(0, Target::Next, refuse);
        asm.load(arg_low(0));
        asm.if_equal(AF_INET, Target::Verdict(Verdict::Allow), refuse);
        return;
    }
    if egress == Egress::TcpPorts {
        // an internet socket must be a TCP stream, the one thing a port rule
        // speaks for; any other family goes on to the checks below
        asm.load(arg_high(0));
        asm.if_equal(0, Target::Next, refuse);
        asm.load(arg_low(0));
        asm.if_equal(AF_INET, Target::Ahead(1), Target::Next);
        asm.if_equal(AF_INET6, Target::Next, Target::Ahead(6));
        asm.load(arg_low(1));
        asm.mask(SOCK_TYPE_MASK);
        asm.if_equal(SOCK_STREAM, Target::Next, refuse);
        asm.load(arg_low(2));
        asm.if_equal(0, Target::Verdict(Verdict::Allow), Target::Next);
        asm.if_equal(IPPROTO_TCP, Target::Verdict(Verdict::Allow), refuse);
    }
    // domain: the high word empty, and none of the families no command needs
    asm.load(arg_high(0));
    asm.if_equal(0, Target::Next, refuse);
    asm.load(arg_low(0));
    asm.if_equal(AF_PACKET, refuse, Target::Next);
    asm.if_equal(AF_BLUETOOTH, refuse, Target::Next);
    asm.if_equal(AF_VSOCK, refuse, Target::Next);
    // netlink: the routing family only, which a resolver may consult
    asm.if_equal(AF_NETLINK, Target::Next, Target::Verdict(Verdict::Allow));
    asm.load(arg_low(2));
    asm.if_equal(NETLINK_ROUTE, Target::Verdict(Verdict::Allow), refuse);
}

static BASELINE: OnceLock<Vec<Instruction>> = OnceLock::new();
static TCP_PORTS: OnceLock<Vec<Instruction>> = OnceLock::new();
static PROXY_ONLY: OnceLock<Vec<Instruction>> = OnceLock::new();

fn slot(egress: Egress) -> &'static OnceLock<Vec<Instruction>> {
    match egress {
        Egress::Open => &BASELINE,
        Egress::TcpPorts => &TCP_PORTS,
        Egress::ProxyOnly => &PROXY_ONLY,
    }
}

/// Builds the program for `egress` if it has not been built yet. Called in
/// the parent, before any fork, so the child finds it ready.
pub fn prepare(egress: Egress) {
    slot(egress).get_or_init(|| assemble(egress));
}

/// Whether this kernel will accept a filter, checked before a run commits to
/// one. `seccomp` can be compiled out or blocked by a container's own policy,
/// and a run whose filter never loaded would be a policy that looks applied
/// and is not.
pub fn available() -> bool {
    // The probe is the call the filter will actually be installed with, not the
    // `seccomp()` syscall: a host can allow one and refuse the other, and a
    // probe that disagrees with the application would refuse runs that would
    // have worked. A null program reaches the same validation and reports
    // EFAULT, where an unsupported mode reports EINVAL — and installs nothing.
    let probed = unsafe {
        libc::prctl(libc::PR_SET_SECCOMP, 2 /* SECCOMP_MODE_FILTER */, std::ptr::null::<Program>())
    };
    probed == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EFAULT)
}

/// Applies the program for `egress` to the calling process. Safe to call after
/// fork: one syscall over a program built before the fork, and no allocation.
///
/// The caller must already have set `PR_SET_NO_NEW_PRIVS`, which the Landlock
/// path does; without it an unprivileged process cannot install a filter.
pub fn restrict_current_process(egress: Egress) -> std::io::Result<()> {
    let Some(code) = slot(egress).get() else {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    };
    let program = Program { len: code.len() as u16, filter: code.as_ptr() };
    // PR_SET_SECCOMP with SECCOMP_MODE_FILTER. Used in place of the seccomp()
    // syscall because this runs single-threaded between fork and exec, so the
    // thread-sync flag that only seccomp() offers has nothing to sync.
    let applied = unsafe {
        libc::prctl(libc::PR_SET_SECCOMP, 2 /* SECCOMP_MODE_FILTER */, &program as *const Program)
    };
    if applied != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
