//! The seccomp programs, run through a small BPF interpreter the way the
//! kernel would run them.

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
fn tcp_ports_leave_an_internet_socket_only_tcp() {
    let code = assemble(Egress::TcpPorts);
    let socket = |domain: u64, kind: u64, proto: u64| evaluate(&code, libc::SYS_socket as u32, [domain, kind, proto, 0, 0, 0]);
    assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, 0), ACTION_ALLOW);
    assert_eq!(socket(AF_INET6 as u64, SOCK_STREAM as u64 | 0x80800, IPPROTO_TCP as u64), ACTION_ALLOW);
    assert_eq!(socket(AF_INET as u64, libc::SOCK_DGRAM as u64, 0), EAFNOSUPPORT);
    assert_eq!(socket(AF_INET6 as u64, libc::SOCK_DGRAM as u64, 0), EAFNOSUPPORT);
    assert_eq!(socket(AF_INET as u64, SOCK_STREAM as u64, 132 /* SCTP */), EAFNOSUPPORT);
    assert_eq!(socket(AF_INET as u64, libc::SOCK_SEQPACKET as u64, 0), EAFNOSUPPORT);
    assert_eq!(socket(libc::AF_UNIX as u64, libc::SOCK_DGRAM as u64, 0), ACTION_ALLOW);
    assert_eq!(socket(AF_NETLINK as u64, libc::SOCK_DGRAM as u64, NETLINK_ROUTE as u64), ACTION_ALLOW);
    assert_eq!(socket(AF_PACKET as u64, libc::SOCK_DGRAM as u64, 0), EAFNOSUPPORT);
    assert_eq!(socket(AF_INET as u64 | (1 << 32), SOCK_STREAM as u64, 0), EAFNOSUPPORT);
}

#[test]
fn a_foreign_architecture_is_killed() {
    let code = assemble(Egress::Open);
    assert_eq!(evaluate_on(&code, AUDIT_ARCH ^ 1, libc::SYS_write as u32, [0; 6]), ACTION_KILL_PROCESS);
}

#[test]
fn every_jump_lands_inside_the_program() {
    for egress in [Egress::Open, Egress::TcpPorts, Egress::ProxyOnly] {
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
