//! Resource ceilings for a native run: the rlimits set between fork and exec,
//! the checks that a ceiling can be set at all, and the supervisor's wait that
//! ends a run at its wall-clock deadline or, on macOS, at its CPU ceiling.
//!
//! Each rlimit is per process and inherited, so every descendant carries it;
//! `--timeout` is the only bound on the run as a whole. A memory ceiling is
//! deliberately absent: an rlimit caps address space, not residency, and a
//! real one needs cgroup v2 in a delegated subtree.

/// One rlimit a request asks for: the kernel's resource id, the value in the
/// kernel's unit, and the flag that asked. `Copy` so the closure that applies
/// them after the fork can own its own.
#[derive(Clone, Copy)]
pub(crate) struct Ceiling {
    pub(crate) resource: libc::c_int,
    pub(crate) value: u64,
    pub(crate) flag: &'static str,
}

pub(crate) const MIB: u64 = 1024 * 1024;

/// The exit code porta reports when a run hit its `--timeout`, the same code
/// `timeout(1)` uses.
pub(crate) const TIMED_OUT: i64 = 124;

/// What a command ended by its CPU ceiling reports: 128 + SIGXCPU, the code
/// the kernel's own signal gives, so the two ways of hitting the ceiling look
/// the same to a script.
pub(crate) const CPU_EXCEEDED: i64 = 128 + libc::SIGXCPU as i64;

/// The hard limit that goes with a soft one. For CPU it sits one second
/// above: Linux skips SIGXCPU and kills outright when the two are equal, while
/// macOS sends SIGXCPU either way, so with the gap both end the command with
/// SIGXCPU at the soft limit (exit 152), and a process that ignores the signal
/// is killed a second later.
fn hard_limit(ceiling: &Ceiling) -> u64 {
    if ceiling.resource == libc::RLIMIT_CPU as libc::c_int { ceiling.value + 1 } else { ceiling.value }
}

/// Sets each ceiling as the soft limit with its hard limit, so the command
/// cannot raise it back. Runs in the child between fork and exec, or in this
/// process right before it replaces itself, and is inherited from there.
pub(crate) fn apply_ceilings(ceilings: &[Ceiling]) -> std::io::Result<()> {
    for ceiling in ceilings {
        let limit = libc::rlimit { rlim_cur: ceiling.value as libc::rlim_t, rlim_max: hard_limit(ceiling) as libc::rlim_t };
        if unsafe { libc::setrlimit(ceiling.resource as _, &limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Why a ceiling cannot be set, if it cannot. An unprivileged process may only
/// lower a hard limit, so a ceiling above this user's is one the kernel would
/// refuse; better to say so here, with the number, than to fail the spawn with
/// a bare `Operation not permitted`.
pub(crate) fn unsettable_ceiling(ceilings: &[Ceiling]) -> Option<String> {
    ceilings.iter().find_map(|ceiling| {
        let mut current = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if unsafe { libc::getrlimit(ceiling.resource as _, &mut current) } != 0 {
            return Some(format!("{} cannot be applied: {}", ceiling.flag, std::io::Error::last_os_error()));
        }
        (current.rlim_max != libc::RLIM_INFINITY && hard_limit(ceiling) > current.rlim_max as u64).then(|| {
            format!(
                "{} asks for {} but this user's hard limit is {}; an unprivileged process can only lower it",
                ceiling.flag, ceiling.value, current.rlim_max
            )
        })
    })
}

/// Ends the whole process group and reaps the child. Negated pid: a shell's
/// children die with it. SIGKILL because a run past its budget has already
/// had its share; then the wait, so no zombie is left.
pub(crate) fn kill_group(child: &mut std::process::Child) {
    let group = child.id() as libc::pid_t;
    unsafe { libc::kill(-group, libc::SIGKILL) };
    let _ = child.wait();
}

/// What a run ended for passing its memory ceiling reports: 128 + SIGKILL,
/// the code the kernel's own OOM kill gives on Linux, so both platforms agree.
pub(crate) const MEMORY_EXCEEDED: i64 = 128 + libc::SIGKILL as i64;

/// Waits for the child, killing its process group at the wall-clock deadline
/// (exit 124) or, on macOS, once the group's CPU time (exit 152) or resident
/// footprint (exit 137) reaches its ceiling. Linux needs neither watch here:
/// past the CPU hard limit the kernel kills, and the memory ceiling is a
/// cgroup the kernel enforces. macOS only ever sends SIGXCPU, which a program
/// may ignore, and has no cgroup, so there the supervisor measures the group
/// every quarter second and does the killing itself.
pub(crate) fn wait_within(mut child: std::process::Child, timeout: u64, max_cpu: u64, max_memory: u64) -> std::io::Result<i64> {
    let deadline = (timeout > 0).then(|| std::time::Instant::now() + std::time::Duration::from_secs(timeout));
    let watch = cfg!(target_os = "macos") && (max_cpu > 0 || max_memory > 0);
    if deadline.is_none() && !watch {
        return child.wait().map(exit_code);
    }
    let group = child.id() as libc::pid_t;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(exit_code(status));
        }
        let past_deadline = deadline.is_some_and(|at| std::time::Instant::now() >= at);
        let over = if past_deadline { Some(TIMED_OUT) } else if watch { ceiling_passed(group, max_cpu, max_memory) } else { None };
        if let Some(code) = over {
            kill_group(&mut child);
            return Ok(code);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Which ceiling the group has passed, if any: the exit code to report.
fn ceiling_passed(group: libc::pid_t, max_cpu: u64, max_memory: u64) -> Option<i64> {
    let usage = group_usage(group);
    if max_cpu > 0 && usage.cpu_seconds >= max_cpu as f64 {
        return Some(CPU_EXCEEDED);
    }
    (max_memory > 0 && usage.memory_bytes >= max_memory).then_some(MEMORY_EXCEEDED)
}

/// A child's exit code, or 128 plus the signal that ended it, as a shell
/// would report it.
pub(crate) fn exit_code(status: std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(code) => code as i64,
        None => 128 + status.signal().unwrap_or(0) as i64,
    }
}

/// What a process group has used so far, as the supervisor sees it.
#[derive(Default)]
struct GroupUsage {
    /// CPU seconds of every live member, plus the time of the children each
    /// has already reaped. A process that exits unreaped takes its share with
    /// it, but every process still carries its own rlimit.
    cpu_seconds: f64,
    /// The physical footprint of every live member, summed: what Activity
    /// Monitor calls Memory. Read at each poll, so a burst can pass the
    /// ceiling for up to a quarter second before the group is ended.
    memory_bytes: u64,
}

#[cfg(target_os = "macos")]
fn group_usage(group: libc::pid_t) -> GroupUsage {
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = vec![0 as libc::pid_t; 4096];
    let pid_size = std::mem::size_of::<libc::pid_t>();
    let mut units: u64 = 0;
    let mut footprint: u64 = 0;
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    // One block: list the group, read each member's usage, fetch the timebase.
    // libc mirrors the header's `rusage_info_t *` although the call takes the
    // struct's address, as every C caller passes it.
    unsafe {
        let bytes = libc::proc_listpids(PROC_PGRP_ONLY, group as u32, pids.as_mut_ptr().cast(), (pids.len() * pid_size) as libc::c_int);
        let count = (bytes.max(0) as usize / pid_size).min(pids.len());
        for &pid in pids[..count].iter().filter(|&&pid| pid > 0) {
            let mut info: libc::rusage_info_v1 = std::mem::zeroed();
            if libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V1, (&mut info as *mut libc::rusage_info_v1).cast()) == 0 {
                units += info.ri_user_time + info.ri_system_time + info.ri_child_user_time + info.ri_child_system_time;
                footprint += info.ri_phys_footprint;
            }
        }
        libc::mach_timebase_info(&mut timebase);
    }
    // The times are in mach absolute-time units; the timebase turns them into
    // nanoseconds (1/1 on Intel, 125/3 on Apple silicon).
    let cpu_seconds = units as f64 * timebase.numer as f64 / timebase.denom.max(1) as f64 / 1e9;
    GroupUsage { cpu_seconds, memory_bytes: footprint }
}

#[cfg(not(target_os = "macos"))]
fn group_usage(_group: libc::pid_t) -> GroupUsage {
    GroupUsage::default()
}
