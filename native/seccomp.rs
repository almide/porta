//! The part of a network policy Landlock cannot express.
//!
//! Landlock's network rules cover TCP bind and connect. They say nothing about
//! UDP, Unix sockets or raw sockets, so proxy mode — whose whole claim is that
//! the loopback proxy is the *only* egress — could not be enforced with
//! Landlock alone, and refused to run rather than apply half a policy.
//!
//! A seccomp filter closes the rest at the one syscall that opens an egress
//! channel. `socket(2)` is refused unless it asks for `AF_INET` with
//! `SOCK_STREAM`; every other domain and type returns `EAFNOSUPPORT`, which is
//! the errno a client already knows how to fall back from. Landlock still
//! decides *which* TCP port may be reached, so the two together are the
//! invariant: one loopback endpoint, no UDP, no Unix sockets.
//!
//! `io_uring` is refused outright, because a ring can open a socket without
//! ever issuing `socket(2)` — a filter that watched only that syscall would
//! claim an egress policy the kernel does not hold. `ENOSYS` is the answer, so
//! a library that probes for it falls back to poll like it would on an older
//! kernel.
//!
//! `socketpair(2)` is deliberately untouched. It makes an anonymous pair both
//! of whose ends the process already holds; it reaches nothing, and runtimes
//! use it for their own plumbing.
#![cfg(target_os = "linux")]

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
const JMP_JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const ALU_AND_K: u16 = 0x54; // BPF_ALU | BPF_AND | BPF_K
const RET_K: u16 = 0x06; // BPF_RET | BPF_K

/// Offsets into `struct seccomp_data`: the syscall number, the architecture
/// token, and the low half of each argument. The arguments are 64-bit and BPF
/// loads 32 bits, so the high half is checked separately — a domain smuggled
/// in the upper word must not read as `AF_INET` down here.
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const OFFSET_ARG0_LOW: u32 = 16;
const OFFSET_ARG0_HIGH: u32 = 20;
const OFFSET_ARG1_LOW: u32 = 24;
const OFFSET_ARG1_HIGH: u32 = 28;

const ACTION_ALLOW: u32 = 0x7fff_0000; // SECCOMP_RET_ALLOW
const ACTION_KILL_PROCESS: u32 = 0x8000_0000; // SECCOMP_RET_KILL_PROCESS
/// SECCOMP_RET_ERRNO | EAFNOSUPPORT
const ACTION_REFUSE: u32 = 0x0005_0000 | (libc::EAFNOSUPPORT as u32);
/// SECCOMP_RET_ERRNO | ENOSYS — this kernel simply does not have that call.
const ACTION_NO_SYSCALL: u32 = 0x0005_0000 | (libc::ENOSYS as u32);

const AF_INET: u32 = libc::AF_INET as u32;
const SOCK_STREAM: u32 = libc::SOCK_STREAM as u32;
/// `type` carries `SOCK_NONBLOCK` and `SOCK_CLOEXEC` in its high bits.
const SOCK_TYPE_MASK: u32 = 0xf;

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e; // AUDIT_ARCH_X86_64
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7; // AUDIT_ARCH_AARCH64

#[cfg(target_arch = "x86_64")]
const SYS_SOCKET: u32 = 41;
#[cfg(target_arch = "aarch64")]
const SYS_SOCKET: u32 = 198;

/// Added after the syscall tables were unified, so it is the same number on
/// every architecture this builds for.
const SYS_IO_URING_SETUP: u32 = 425;

fn load(offset: u32) -> Instruction {
    Instruction { code: LD_W_ABS, jt: 0, jf: 0, k: offset }
}

/// Continue to the next instruction when the word equals `value`, otherwise
/// jump `otherwise` instructions forward.
fn equals(value: u32, otherwise: u8) -> Instruction {
    Instruction { code: JMP_JEQ_K, jt: 0, jf: otherwise, k: value }
}

/// Jump `then` instructions forward when the word equals `value`, otherwise
/// continue to the next instruction.
fn equals_jump(value: u32, then: u8) -> Instruction {
    Instruction { code: JMP_JEQ_K, jt: then, jf: 0, k: value }
}

fn returns(action: u32) -> Instruction {
    Instruction { code: RET_K, jt: 0, jf: 0, k: action }
}

/// The filter: refuse `socket(2)` for anything but a TCP/IPv4 socket, and let
/// every other syscall through untouched.
///
/// A process running under a different architecture's syscall table would read
/// syscall numbers from the wrong map, so a mismatched `arch` kills the process
/// rather than guessing — the one case where an errno would be a lie.
/// A jump counts from the instruction after it, so every distance here is
/// measured against the eighteen-instruction whole: ALLOW at 14, REFUSE at 15,
/// NO_SYSCALL at 16, KILL at 17. Inserting an instruction means recounting all
/// of them.
fn filter() -> [Instruction; 14] {
    [
        // 0,1: arch must be the one these syscall numbers belong to
        load(OFFSET_ARCH),
        equals(AUDIT_ARCH, 15),
        // 2,3: a ring can open a socket without socket(2), so it does not open
        load(OFFSET_NR),
        equals_jump(SYS_IO_URING_SETUP, 12),
        // 4: every other syscall but socket() is none of this filter's business
        equals(SYS_SOCKET, 9),
        // 5-8: domain — the high word empty and the low word AF_INET
        load(OFFSET_ARG0_HIGH),
        equals(0, 8),
        load(OFFSET_ARG0_LOW),
        equals(AF_INET, 6),
        // 9-13: type — the high word empty, and SOCK_STREAM exactly, once the
        // SOCK_NONBLOCK and SOCK_CLOEXEC bits are masked off
        load(OFFSET_ARG1_HIGH),
        equals(0, 4),
        load(OFFSET_ARG1_LOW),
        Instruction { code: ALU_AND_K, jt: 0, jf: 0, k: SOCK_TYPE_MASK },
        equals(SOCK_STREAM, 1),
    ]
}

/// The instructions the jumps above land on. Kept next to `filter` because the
/// jump distances there are counted against this exact tail.
fn tail() -> [Instruction; 4] {
    [
        returns(ACTION_ALLOW),
        returns(ACTION_REFUSE),
        returns(ACTION_NO_SYSCALL),
        returns(ACTION_KILL_PROCESS),
    ]
}

/// Whether this kernel will accept the filter, checked before a run commits to
/// it. `seccomp` can be compiled out or blocked by a container's own policy,
/// and a proxy-mode run whose filter never loaded would be a policy that looks
/// applied and is not.
pub fn available() -> bool {
    // SECCOMP_SET_MODE_FILTER with a null program: the kernel validates the
    // mode and reports EFAULT, where an unsupported mode reports EINVAL.
    let result = unsafe {
        libc::syscall(libc::SYS_seccomp, 1 /* SET_MODE_FILTER */, 0, std::ptr::null::<Program>())
    };
    result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EFAULT)
}

/// Applies the filter to the calling process. Safe to call after fork: one
/// syscall over a program built before the fork, and no allocation.
///
/// The caller must already have set `PR_SET_NO_NEW_PRIVS`, which the Landlock
/// path does; without it an unprivileged process cannot install a filter.
pub fn restrict_current_process() -> std::io::Result<()> {
    let head = filter();
    let rest = tail();
    let mut instructions = [Instruction { code: 0, jt: 0, jf: 0, k: 0 }; 18];
    instructions[..head.len()].copy_from_slice(&head);
    instructions[head.len()..].copy_from_slice(&rest);
    let program = Program { len: instructions.len() as u16, filter: instructions.as_ptr() };
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
