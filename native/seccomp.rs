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

/// One BPF instruction, as the kernel's `sock_filter` lays it out.
#[repr(C)]
#[derive(Clone, Copy)]
struct Instruction {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct Program {
    len: u16,
    filter: *const Instruction,
}

const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JMP_JA: u16 = 0x05; // BPF_JMP | BPF_JA
const JMP_JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const JMP_JSET_K: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
const ALU_AND_K: u16 = 0x54; // BPF_ALU | BPF_AND | BPF_K
const RET_K: u16 = 0x06; // BPF_RET | BPF_K

/// Offsets into `struct seccomp_data`: the syscall number, the architecture
/// token, and each 64-bit argument as two 32-bit halves. BPF loads 32 bits,
/// so a value smuggled in the upper word must be checked separately where it
/// would matter.
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const fn arg_low(index: u32) -> u32 { 16 + index * 8 }
const fn arg_high(index: u32) -> u32 { 20 + index * 8 }

const ACTION_ALLOW: u32 = 0x7fff_0000; // SECCOMP_RET_ALLOW
const ACTION_KILL_PROCESS: u32 = 0x8000_0000; // SECCOMP_RET_KILL_PROCESS
const ACTION_ERRNO: u32 = 0x0005_0000; // SECCOMP_RET_ERRNO

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
    /// baseline closes the families a TCP rule cannot see.
    TcpPorts,
    /// The loopback proxy is the only permitted egress.
    ProxyOnly,
}

/// Where a jump lands: the next instruction, one of the verdicts at the end
/// of the program, or a fixed number of instructions ahead.
#[derive(Clone, Copy)]
enum Target {
    Next,
    Verdict(Verdict),
    Ahead(u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allow,
    Eperm,
    Enosys,
    Eafnosupport,
    Eprotonosupport,
    Kill,
}

const VERDICTS: [Verdict; 6] = [
    Verdict::Allow,
    Verdict::Eperm,
    Verdict::Enosys,
    Verdict::Eafnosupport,
    Verdict::Eprotonosupport,
    Verdict::Kill,
];

impl Verdict {
    fn action(self) -> u32 {
        match self {
            Verdict::Allow => ACTION_ALLOW,
            Verdict::Eperm => ACTION_ERRNO | libc::EPERM as u32,
            Verdict::Enosys => ACTION_ERRNO | libc::ENOSYS as u32,
            Verdict::Eafnosupport => ACTION_ERRNO | libc::EAFNOSUPPORT as u32,
            Verdict::Eprotonosupport => ACTION_ERRNO | libc::EPROTONOSUPPORT as u32,
            Verdict::Kill => ACTION_KILL_PROCESS,
        }
    }

    fn index(self) -> usize {
        // VERDICTS lists every variant, so a position always exists; 0 is the
        // Allow verdict, a safe floor if one were ever missing.
        VERDICTS.iter().position(|verdict| *verdict == self).unwrap_or(0)
    }
}

/// Builds a program whose jumps name their destinations; the distances are
/// measured when the program is finished, so they cannot disagree with the
/// layout.
struct Assembler {
    code: Vec<Instruction>,
    jumps: Vec<(Target, Target)>,
}

impl Assembler {
    fn new() -> Self {
        Assembler { code: Vec::with_capacity(160), jumps: Vec::with_capacity(160) }
    }

    fn emit(&mut self, code: u16, k: u32, jt: Target, jf: Target) {
        self.code.push(Instruction { code, jt: 0, jf: 0, k });
        self.jumps.push((jt, jf));
    }

    fn load(&mut self, offset: u32) {
        self.emit(LD_W_ABS, offset, Target::Next, Target::Next);
    }

    fn mask(&mut self, bits: u32) {
        self.emit(ALU_AND_K, bits, Target::Next, Target::Next);
    }

    /// If the accumulator equals `value`, go to `then`; otherwise `otherwise`.
    fn if_equal(&mut self, value: u32, then: Target, otherwise: Target) {
        self.emit(JMP_JEQ_K, value, then, otherwise);
    }

    /// If the accumulator has any of `bits` set, go to `then`; otherwise `otherwise`.
    fn if_any(&mut self, bits: u32, then: Target, otherwise: Target) {
        self.emit(JMP_JSET_K, bits, then, otherwise);
    }

    fn jump(&mut self, target: Target) {
        // JA carries its distance in k; the verdict is resolved like the others.
        self.emit(JMP_JA, 0, target, target);
    }

    /// Refuse every syscall in `numbers` with `verdict`. The accumulator must
    /// hold the syscall number.
    fn refuse_each(&mut self, numbers: &[u32], verdict: Verdict) {
        for number in numbers {
            self.if_equal(*number, Target::Verdict(verdict), Target::Next);
        }
    }

    /// Run `body` only for syscall `number`; the accumulator holds the number
    /// before and after. `body` must end by jumping to a verdict.
    fn for_syscall(&mut self, number: u32, body: impl FnOnce(&mut Assembler)) {
        let guard = self.code.len();
        self.emit(JMP_JEQ_K, number, Target::Next, Target::Ahead(0));
        body(self);
        self.load(OFFSET_NR);
        // The reload above is what a skipped block lands on; it costs one
        // instruction and keeps every following check honest.
        let body = self.code.len() - guard - 2;
        assert!(body <= u8::MAX as usize, "block too long for one jump");
        self.jumps[guard].1 = Target::Ahead(body as u8);
    }

    fn finish(mut self) -> Vec<Instruction> {
        let first_verdict = self.code.len();
        for verdict in VERDICTS {
            self.code.push(Instruction { code: RET_K, jt: 0, jf: 0, k: verdict.action() });
            self.jumps.push((Target::Next, Target::Next));
        }
        for (index, (jt, jf)) in self.jumps.iter().enumerate() {
            let distance = |target: &Target| -> u8 {
                match target {
                    Target::Next => 0,
                    Target::Ahead(count) => *count,
                    Target::Verdict(verdict) => {
                        let landing = first_verdict + verdict.index();
                        let distance = landing - index - 1;
                        assert!(distance <= u8::MAX as usize, "verdict out of jump range");
                        distance as u8
                    }
                }
            };
            let instruction = &mut self.code[index];
            if instruction.code == JMP_JA {
                instruction.k = distance(jt) as u32;
            } else {
                instruction.jt = distance(jt);
                instruction.jf = distance(jf);
            }
        }
        self.code
    }
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
static PROXY_ONLY: OnceLock<Vec<Instruction>> = OnceLock::new();

fn slot(egress: Egress) -> &'static OnceLock<Vec<Instruction>> {
    match egress {
        Egress::Open | Egress::TcpPorts => &BASELINE,
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
mod tests {
    use super::*;

    /// Runs a program the way the kernel would for one `seccomp_data`, and
    /// reports the action it returns.
    fn evaluate(code: &[Instruction], nr: u32, args: [u64; 6]) -> u32 {
        evaluate_on(code, AUDIT_ARCH, nr, args)
    }

    fn evaluate_on(code: &[Instruction], arch: u32, nr: u32, args: [u64; 6]) -> u32 {
        let mut data = [0u32; 16];
        data[0] = nr;
        data[1] = arch;
        for (index, arg) in args.iter().enumerate() {
            data[4 + index * 2] = (*arg & 0xffff_ffff) as u32;
            data[5 + index * 2] = (*arg >> 32) as u32;
        }
        let mut acc = 0u32;
        let mut pc = 0usize;
        loop {
            let ins = code[pc];
            pc += 1;
            match ins.code {
                LD_W_ABS => acc = data[(ins.k / 4) as usize],
                ALU_AND_K => acc &= ins.k,
                JMP_JA => pc += ins.k as usize,
                JMP_JEQ_K => pc += if acc == ins.k { ins.jt } else { ins.jf } as usize,
                JMP_JSET_K => pc += if acc & ins.k != 0 { ins.jt } else { ins.jf } as usize,
                RET_K => return ins.k,
                other => panic!("unknown opcode {other:#x}"),
            }
        }
    }

    const EPERM: u32 = ACTION_ERRNO | libc::EPERM as u32;
    const ENOSYS: u32 = ACTION_ERRNO | libc::ENOSYS as u32;
    const EAFNOSUPPORT: u32 = ACTION_ERRNO | libc::EAFNOSUPPORT as u32;

    #[test]
    fn baseline_lets_ordinary_syscalls_through() {
        let code = assemble(Egress::Open);
        assert_eq!(evaluate(&code, libc::SYS_openat as u32, [0; 6]), ACTION_ALLOW);
        assert_eq!(evaluate(&code, libc::SYS_write as u32, [0; 6]), ACTION_ALLOW);
        assert_eq!(evaluate(&code, libc::SYS_clone as u32, [0x1200011, 0, 0, 0, 0, 0]), ACTION_ALLOW);
        assert_eq!(evaluate(&code, libc::SYS_ioctl as u32, [0, 0x5401 /* TCGETS */, 0, 0, 0, 0]), ACTION_ALLOW);
        assert_eq!(evaluate(&code, libc::SYS_execveat as u32, [3, 0, 0, 0, 0, 0]), ACTION_ALLOW);
    }

    #[test]
    fn baseline_refuses_the_bypass_primitives() {
        let code = assemble(Egress::Open);
        assert_eq!(evaluate(&code, libc::SYS_ptrace as u32, [0; 6]), EPERM);
        assert_eq!(evaluate(&code, libc::SYS_mount as u32, [0; 6]), EPERM);
        assert_eq!(evaluate(&code, SYS_IO_URING_SETUP, [0; 6]), ENOSYS);
        assert_eq!(evaluate(&code, SYS_CLONE3, [0; 6]), ENOSYS);
        assert_eq!(evaluate(&code, libc::SYS_clone as u32, [0x1000_0000, 0, 0, 0, 0, 0]), EPERM);
        assert_eq!(evaluate(&code, libc::SYS_ioctl as u32, [0, TIOCSTI as u64, 0, 0, 0, 0]), EPERM);
        assert_eq!(evaluate(&code, libc::SYS_execveat as u32, [3, 0, 0, 0, AT_EMPTY_PATH as u64, 0]), EPERM);
    }

    #[test]
    fn baseline_closes_the_families_a_tcp_rule_cannot_see() {
        let code = assemble(Egress::Open);
        let socket = |domain: u64, kind: u64, proto: u64| evaluate(&code, libc::SYS_socket as u32, [domain, kind, proto, 0, 0, 0]);
        assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, 0), ACTION_ALLOW);
        assert_eq!(socket(libc::AF_INET6 as u64, libc::SOCK_DGRAM as u64, 0), ACTION_ALLOW);
        assert_eq!(socket(libc::AF_UNIX as u64, SOCK_STREAM as u64 | 0x80000, 0), ACTION_ALLOW);
        assert_eq!(socket(AF_NETLINK as u64, libc::SOCK_RAW as u64, NETLINK_ROUTE as u64), EAFNOSUPPORT, "raw wins over netlink route");
        assert_eq!(socket(AF_NETLINK as u64, libc::SOCK_DGRAM as u64, NETLINK_ROUTE as u64), ACTION_ALLOW);
        assert_eq!(socket(AF_NETLINK as u64, libc::SOCK_DGRAM as u64, 15), EAFNOSUPPORT);
        assert_eq!(socket(AF_PACKET as u64, libc::SOCK_DGRAM as u64, 0), EAFNOSUPPORT);
        assert_eq!(socket(AF_VSOCK as u64, SOCK_STREAM as u64, 0), EAFNOSUPPORT);
        assert_eq!(socket(AF_INET as u64, SOCK_RAW as u64, 1), EAFNOSUPPORT);
        assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, IPPROTO_MPTCP as u64), ACTION_ERRNO | libc::EPROTONOSUPPORT as u32);
        assert_eq!(socket(AF_INET as u64 | (1 << 32), SOCK_STREAM as u64, 0), EAFNOSUPPORT, "domain smuggled in the high word");
    }

    #[test]
    fn proxy_only_permits_one_socket_shape() {
        let code = assemble(Egress::ProxyOnly);
        let socket = |domain: u64, kind: u64, proto: u64| evaluate(&code, libc::SYS_socket as u32, [domain, kind, proto, 0, 0, 0]);
        assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, 0), ACTION_ALLOW);
        assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64 | 0x80800, 6), ACTION_ALLOW);
        assert_eq!(socket(AF_INET as u64, libc::SOCK_DGRAM as u64, 0), EAFNOSUPPORT);
        assert_eq!(socket(libc::AF_UNIX as u64, SOCK_STREAM as u64, 0), EAFNOSUPPORT);
        assert_eq!(socket(libc::AF_INET6 as u64, SOCK_STREAM as u64, 0), EAFNOSUPPORT);
        assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, IPPROTO_MPTCP as u64), ACTION_ERRNO | libc::EPROTONOSUPPORT as u32);
        assert_eq!(evaluate(&code, libc::SYS_ptrace as u32, [0; 6]), EPERM);
    }

    #[test]
    fn a_foreign_architecture_is_killed() {
        let code = assemble(Egress::Open);
        assert_eq!(evaluate_on(&code, AUDIT_ARCH ^ 1, libc::SYS_write as u32, [0; 6]), ACTION_KILL_PROCESS);
    }

    #[test]
    fn every_jump_lands_inside_the_program() {
        for egress in [Egress::Open, Egress::ProxyOnly] {
            let code = assemble(egress);
            for (index, ins) in code.iter().enumerate() {
                let reach = |offset: usize| assert!(index + 1 + offset < code.len(), "jump past the end at {index}");
                match ins.code {
                    JMP_JA => reach(ins.k as usize),
                    JMP_JEQ_K | JMP_JSET_K => { reach(ins.jt as usize); reach(ins.jf as usize); }
                    _ => {}
                }
            }
            assert!(code.len() < 4096);
        }
    }
}
