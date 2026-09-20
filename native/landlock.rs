//! Linux native enforcement through Landlock.
//!
//! The ruleset is built in the parent and only applied in the child, so the
//! post-fork path makes two syscalls and allocates nothing. Rules the running
//! kernel cannot express are refused rather than skipped: a policy that is not
//! applied must fail the run, never run it unrestricted.
#![cfg(target_os = "linux")]

const SYS_CREATE_RULESET: libc::c_long = 444;
const SYS_ADD_RULE: libc::c_long = 445;
const SYS_RESTRICT_SELF: libc::c_long = 446;

const CREATE_RULESET_VERSION: u32 = 1;
const RULE_PATH_BENEATH: libc::c_long = 1;
const RULE_NET_PORT: libc::c_long = 2;

const ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

/// Reads, as understood by Landlock ABI 1. Handling these closes every path
/// that no rule names, which is the only shape an allow-list can take.
const READ_RIGHTS_ABI1: u64 = (1 << 0)   // EXECUTE
    | (1 << 2)   // READ_FILE
    | (1 << 3);  // READ_DIR

/// Filesystem writes, as understood by Landlock ABI 1.
const WRITE_RIGHTS_ABI1: u64 = (1 << 1)   // WRITE_FILE
    | (1 << 4)   // REMOVE_DIR
    | (1 << 5)   // REMOVE_FILE
    | (1 << 6)   // MAKE_CHAR
    | (1 << 7)   // MAKE_DIR
    | (1 << 8)   // MAKE_REG
    | (1 << 9)   // MAKE_SOCK
    | (1 << 10)  // MAKE_FIFO
    | (1 << 11)  // MAKE_BLOCK
    | (1 << 12); // MAKE_SYM
/// TRUNCATE exists from ABI 3; requesting it on an older kernel is rejected.
const ACCESS_FS_TRUNCATE: u64 = 1 << 14;

/// ABI that first understands network rules.
const MIN_ABI_FOR_NET: i64 = 4;
/// ABI that first understands truncation as a distinct right.
const MIN_ABI_FOR_TRUNCATE: i64 = 3;

/// The only place a landlock syscall is issued. The arguments travel as one
/// array in syscall order, and a caller that has a pointer casts it, so the
/// order here cannot drift from the kernel's. Each attribute struct outlives
/// its call and the kernel copies it before returning, so no borrow escapes.
fn landlock_syscall(operation: libc::c_long, args: [libc::c_long; 4]) -> libc::c_long {
    unsafe { libc::syscall(operation, args[0], args[1], args[2], args[3]) }
}

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[repr(C)]
struct NetPortAttr {
    allowed_access: u64,
    port: u64,
}

/// What the caller asked the kernel to enforce.
pub struct Policy {
    pub writable_dirs: Vec<String>,
    /// Roots the caller was granted read access to. A path here that cannot be
    /// opened refuses the run, exactly as a write grant does.
    pub readable_dirs: Vec<String>,
    /// The platform's own directories. One that does not exist on this host is
    /// skipped rather than refused — it is not something the caller asked for,
    /// and `/lib64` is absent on arm64 Debian.
    pub system_dirs: Vec<String>,
    pub restrict_reads: bool,
    pub tcp_ports: Vec<u16>,
    pub restrict_network: bool,
}

/// A prepared ruleset. The descriptor is owned, so it closes on every path.
pub struct Ruleset {
    fd: std::os::fd::OwnedFd,
}

impl Ruleset {
    pub fn descriptor(&self) -> i32 {
        std::os::fd::AsRawFd::as_raw_fd(&self.fd)
    }

    /// Applies the ruleset to the calling process. Safe to call after fork:
    /// two syscalls, no allocation, no locks.
    pub fn restrict_current_process(fd: i32) -> std::io::Result<()> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if landlock_syscall(SYS_RESTRICT_SELF, [fd as libc::c_long, 0, 0, 0]) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Landlock ABI the running kernel reports, or an error when it has none.
pub fn abi_version() -> Result<i64, String> {
    // attr = NULL, size = 0, flags = VERSION
    let abi = landlock_syscall(SYS_CREATE_RULESET, [0, 0, CREATE_RULESET_VERSION as libc::c_long, 0]);
    if abi < 1 {
        return Err(format!(
            "this kernel has no usable Landlock support ({}); \
             porta will not run a native command it cannot restrict",
            std::io::Error::last_os_error()
        ));
    }
    Ok(abi)
}

fn write_rights(abi: i64) -> u64 {
    if abi >= MIN_ABI_FOR_TRUNCATE {
        WRITE_RIGHTS_ABI1 | ACCESS_FS_TRUNCATE
    } else {
        WRITE_RIGHTS_ABI1
    }
}

fn create_ruleset(handled_fs: u64, handled_net: u64) -> Result<Ruleset, String> {
    let attr = RulesetAttr {
        handled_access_fs: handled_fs,
        handled_access_net: handled_net,
    };
    // attr, size, flags — in that order, as the kernel declares them.
    let fd = landlock_syscall(SYS_CREATE_RULESET, [
        &attr as *const RulesetAttr as libc::c_long,
        std::mem::size_of::<RulesetAttr>() as libc::c_long,
        0,
        0,
    ]);
    if fd < 0 {
        return Err(format!("Landlock ruleset refused: {}", std::io::Error::last_os_error()));
    }
    // The kernel just handed this descriptor over and nothing else holds it.
    let owned = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd as i32) };
    Ok(Ruleset { fd: owned })
}

fn add_rule(ruleset_fd: i32, rule_type: libc::c_long, attr: libc::c_long) -> libc::c_long {
    landlock_syscall(SYS_ADD_RULE, [ruleset_fd as libc::c_long, rule_type, attr, 0])
}

fn allow_directory(ruleset: &Ruleset, dir: &str, rights: u64) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    // A mount that cannot be opened cannot be granted. Refusing here keeps the
    // policy honest instead of silently narrowing it. The handle owns the
    // descriptor, so no exit path from here leaks it.
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_CLOEXEC)
        .open(dir)
        .map_err(|error| format!("cannot open mount {}: {}", dir, error))?;
    let attr = PathBeneathAttr {
        allowed_access: rights,
        parent_fd: std::os::fd::AsRawFd::as_raw_fd(&handle),
    };
    let added = add_rule(ruleset.descriptor(), RULE_PATH_BENEATH, &attr as *const PathBeneathAttr as libc::c_long);
    if added != 0 {
        return Err(format!("cannot grant writes beneath {}: {}", dir, std::io::Error::last_os_error()));
    }
    Ok(())
}

fn allow_tcp_port(ruleset: &Ruleset, port: u16) -> Result<(), String> {
    let attr = NetPortAttr { allowed_access: ACCESS_NET_CONNECT_TCP, port: port as u64 };
    let added = add_rule(ruleset.descriptor(), RULE_NET_PORT, &attr as *const NetPortAttr as libc::c_long);
    if added != 0 {
        return Err(format!("cannot allow TCP port {}: {}", port, std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Builds the ruleset for a policy, or explains which rule this kernel refuses.
pub fn prepare(policy: &Policy) -> Result<Ruleset, String> {
    let abi = abi_version()?;
    if policy.restrict_network && abi < MIN_ABI_FOR_NET {
        return Err(format!(
            "--allow-net needs Landlock ABI {} for network rules, this kernel reports {}; \
             porta will not run the command with the network open instead",
            MIN_ABI_FOR_NET, abi
        ));
    }
    let writes = write_rights(abi);
    let reads = if policy.restrict_reads { READ_RIGHTS_ABI1 } else { 0 };
    let handled_net = if policy.restrict_network { ACCESS_NET_CONNECT_TCP } else { 0 };
    let ruleset = create_ruleset(writes | reads, handled_net)?;
    for dir in &policy.writable_dirs {
        allow_directory(&ruleset, dir, writes | reads)?;
    }
    // Reading is handled all-or-nothing: with reads restricted, a path no rule
    // names is closed, so the platform's own directories have to be named too.
    if policy.restrict_reads {
        for dir in &policy.readable_dirs {
            allow_directory(&ruleset, dir, reads)?;
        }
        for dir in policy.system_dirs.iter().filter(|dir| std::path::Path::new(dir).exists()) {
            allow_directory(&ruleset, dir, reads)?;
        }
    }
    for port in &policy.tcp_ports {
        allow_tcp_port(&ruleset, *port)?;
    }
    Ok(ruleset)
}
