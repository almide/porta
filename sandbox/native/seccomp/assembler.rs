//! The BPF a seccomp program is made of, and an assembler whose jumps name
//! their destinations rather than count them.

/// One BPF instruction, as the kernel's `sock_filter` lays it out.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Instruction {
    pub(super) code: u16,
    pub(super) jt: u8,
    pub(super) jf: u8,
    pub(super) k: u32,
}

#[repr(C)]
pub(super) struct Program {
    pub(super) len: u16,
    pub(super) filter: *const Instruction,
}

pub(super) const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
pub(super) const JMP_JA: u16 = 0x05; // BPF_JMP | BPF_JA
pub(super) const JMP_JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
pub(super) const JMP_JSET_K: u16 = 0x45; // BPF_JMP | BPF_JSET | BPF_K
pub(super) const ALU_AND_K: u16 = 0x54; // BPF_ALU | BPF_AND | BPF_K
pub(super) const RET_K: u16 = 0x06; // BPF_RET | BPF_K

/// Offsets into `struct seccomp_data`: the syscall number, the architecture
/// token, and each 64-bit argument as two 32-bit halves. BPF loads 32 bits,
/// so a value smuggled in the upper word must be checked separately where it
/// would matter.
pub(super) const OFFSET_NR: u32 = 0;
pub(super) const OFFSET_ARCH: u32 = 4;
pub(super) const fn arg_low(index: u32) -> u32 { 16 + index * 8 }
pub(super) const fn arg_high(index: u32) -> u32 { 20 + index * 8 }

pub(super) const ACTION_ALLOW: u32 = 0x7fff_0000; // SECCOMP_RET_ALLOW
pub(super) const ACTION_KILL_PROCESS: u32 = 0x8000_0000; // SECCOMP_RET_KILL_PROCESS
pub(super) const ACTION_ERRNO: u32 = 0x0005_0000; // SECCOMP_RET_ERRNO

/// Where a jump lands: the next instruction, one of the verdicts at the end
/// of the program, or a fixed number of instructions ahead.
#[derive(Clone, Copy)]
pub(super) enum Target {
    Next,
    Verdict(Verdict),
    Ahead(u8),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Allow,
    Eperm,
    Enosys,
    Eafnosupport,
    Eprotonosupport,
    Kill,
}

pub(super) const VERDICTS: [Verdict; 6] = [
    Verdict::Allow,
    Verdict::Eperm,
    Verdict::Enosys,
    Verdict::Eafnosupport,
    Verdict::Eprotonosupport,
    Verdict::Kill,
];

impl Verdict {
    pub(super) fn action(self) -> u32 {
        match self {
            Verdict::Allow => ACTION_ALLOW,
            Verdict::Eperm => ACTION_ERRNO | libc::EPERM as u32,
            Verdict::Enosys => ACTION_ERRNO | libc::ENOSYS as u32,
            Verdict::Eafnosupport => ACTION_ERRNO | libc::EAFNOSUPPORT as u32,
            Verdict::Eprotonosupport => ACTION_ERRNO | libc::EPROTONOSUPPORT as u32,
            Verdict::Kill => ACTION_KILL_PROCESS,
        }
    }

    pub(super) fn index(self) -> usize {
        // VERDICTS lists every variant, so a position always exists; 0 is the
        // Allow verdict, a safe floor if one were ever missing.
        VERDICTS.iter().position(|verdict| *verdict == self).unwrap_or(0)
    }
}

/// Builds a program whose jumps name their destinations; the distances are
/// measured when the program is finished, so they cannot disagree with the
/// layout.
pub(super) struct Assembler {
    pub(super) code: Vec<Instruction>,
    jumps: Vec<(Target, Target)>,
}

impl Assembler {
    pub(super) fn new() -> Self {
        Assembler { code: Vec::with_capacity(160), jumps: Vec::with_capacity(160) }
    }

    /// One instruction, and where it jumps when true and when false.
    pub(super) fn emit(&mut self, code: u16, k: u32, jumps: (Target, Target)) {
        self.code.push(Instruction { code, jt: 0, jf: 0, k });
        self.jumps.push(jumps);
    }

    pub(super) fn load(&mut self, offset: u32) {
        self.emit(LD_W_ABS, offset, (Target::Next, Target::Next));
    }

    pub(super) fn mask(&mut self, bits: u32) {
        self.emit(ALU_AND_K, bits, (Target::Next, Target::Next));
    }

    /// If the accumulator equals `value`, go to `then`; otherwise `otherwise`.
    pub(super) fn if_equal(&mut self, value: u32, then: Target, otherwise: Target) {
        self.emit(JMP_JEQ_K, value, (then, otherwise));
    }

    /// If the accumulator has any of `bits` set, go to `then`; otherwise `otherwise`.
    pub(super) fn if_any(&mut self, bits: u32, then: Target, otherwise: Target) {
        self.emit(JMP_JSET_K, bits, (then, otherwise));
    }

    pub(super) fn jump(&mut self, target: Target) {
        // JA carries its distance in k; the verdict is resolved like the others.
        self.emit(JMP_JA, 0, (target, target));
    }

    /// Refuse every syscall in `numbers` with `verdict`. The accumulator must
    /// hold the syscall number.
    pub(super) fn refuse_each(&mut self, numbers: &[u32], verdict: Verdict) {
        for number in numbers {
            self.if_equal(*number, Target::Verdict(verdict), Target::Next);
        }
    }

    /// Run `body` only for syscall `number`; the accumulator holds the number
    /// before and after. `body` must end by jumping to a verdict.
    pub(super) fn for_syscall(&mut self, number: u32, body: impl FnOnce(&mut Assembler)) {
        let guard = self.code.len();
        self.emit(JMP_JEQ_K, number, (Target::Next, Target::Ahead(0)));
        body(self);
        self.load(OFFSET_NR);
        // The reload above is what a skipped block lands on; it costs one
        // instruction and keeps every following check honest.
        let body = self.code.len() - guard - 2;
        assert!(body <= u8::MAX as usize, "block too long for one jump");
        self.jumps[guard].1 = Target::Ahead(body as u8);
    }

    pub(super) fn finish(mut self) -> Vec<Instruction> {
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
