//! Linux native enforcement through Landlock.
//!
//! The ruleset is built in the parent and only applied in the child, so the
//! post-fork path makes a handful of syscalls and allocates nothing. Rules the
//! running kernel cannot express are refused rather than skipped: a policy
//! that is not applied must fail the run, never run it unrestricted.
#![cfg(target_os = "linux")]

const SYS_CREATE_RULESET: libc::c_long = 444;
const SYS_ADD_RULE: libc::c_long = 445;
const SYS_RESTRICT_SELF: libc::c_long = 446;

const CREATE_RULESET_VERSION: u32 = 1;
const RULE_PATH_BENEATH: libc::c_long = 1;
const RULE_NET_PORT: libc::c_long = 2;

const ACCESS_NET_BIND_TCP: u64 = 1 << 0;
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
/// Linking or renaming across directories, from ABI 2. Once a kernel knows
/// this right, a ruleset that does not handle it denies every cross-directory
/// rename and link — `mv a/x b/` inside a mount degrades to copy-and-delete,
/// and `ln` fails with EXDEV. It is handled and granted wherever writes are.
const ACCESS_FS_REFER: u64 = 1 << 13;
/// TRUNCATE exists from ABI 3; requesting it on an older kernel is rejected.
const ACCESS_FS_TRUNCATE: u64 = 1 << 14;

/// The rights a rule on a single file may carry. A directory right on a file
/// rule is rejected by the kernel, so file rules are masked to these.
const FILE_RIGHTS: u64 = (1 << 0) | (1 << 1) | (1 << 2) | ACCESS_FS_TRUNCATE;

/// From ABI 6 a domain can be scoped: the sandboxed process cannot send a
/// signal to a process outside it, nor connect to an abstract Unix socket
/// another process outside it listens on. Both are channels to the caller's
/// other processes that no file or port rule would ever name.
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const SCOPE_SIGNAL: u64 = 1 << 1;

/// The first ABI that understands network rules, cross-directory rename and
/// link, truncation as a distinct right, and signal and abstract-socket scoping.
const MIN_ABI_FOR_NET: i64 = 4;
const MIN_ABI_FOR_REFER: i64 = 2;
const MIN_ABI_FOR_TRUNCATE: i64 = 3;
const MIN_ABI_FOR_SCOPE: i64 = 6;

/// The only place a landlock syscall is issued. The arguments travel as one
/// array in syscall order, and a caller that has a pointer casts it, so the
/// order here cannot drift from the kernel's. Each attribute struct outlives
/// its call and the kernel copies it before returning, so no borrow escapes.
fn landlock_syscall(operation: libc::c_long, args: [libc::c_long; 4]) -> libc::c_long {
    unsafe { libc::syscall(operation, args[0], args[1], args[2], args[3]) }
}

/// The ruleset attributes as ABI 6 lays them out. Older kernels are handed
/// only the first two words, which is the size they know.
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

const RULESET_ATTR_SIZE_ABI1: usize = 16;
const RULESET_ATTR_SIZE_ABI6: usize = 24;

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
    /// Single files under `/etc` a command needs to start. Skipped when absent,
    /// for the same reason as the directories.
    pub system_files: Vec<String>,
    pub restrict_reads: bool,
    pub tcp_ports: Vec<u16>,
    /// Ports the command may listen on. With a network rule in force every
    /// other port is closed to `bind(2)`.
    pub bind_ports: Vec<u16>,
    pub restrict_network: bool,
    /// Under open reads, paths closed anyway (the preset, `--deny-read`) when
    /// no mount namespace covers them. Reads are then handled after all, and
    /// granted everywhere but beneath these: see [`open_except`].
    pub open_reads_except: Vec<String>,
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

// `/proc/self` is not granted under a strict read policy, and this is where
// the reason is recorded. A rule added by the process that then execs in
// place does grant its own entry — measured — but `/proc/self` names whoever
// opens it, so the shell that is porta's usual command gets its entry and
// every tool the shell starts gets nothing, since each has a pid of its own.
// A grant that covers the wrapper and not the work misleads, so `/proc` stays
// closed as a whole, and a command that needs it takes it as a mount.

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
    let mut rights = WRITE_RIGHTS_ABI1;
    if abi >= MIN_ABI_FOR_REFER { rights |= ACCESS_FS_REFER; }
    if abi >= MIN_ABI_FOR_TRUNCATE { rights |= ACCESS_FS_TRUNCATE; }
    rights
}

fn create_ruleset(abi: i64, handled_fs: u64, handled_net: u64) -> Result<Ruleset, String> {
    let scoped = if abi >= MIN_ABI_FOR_SCOPE { SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL } else { 0 };
    let size = if abi >= MIN_ABI_FOR_SCOPE { RULESET_ATTR_SIZE_ABI6 } else { RULESET_ATTR_SIZE_ABI1 };
    let attr = RulesetAttr { handled_access_fs: handled_fs, handled_access_net: handled_net, scoped };
    // attr, size, flags — in that order, as the kernel declares them.
    let fd = landlock_syscall(SYS_CREATE_RULESET, [
        &attr as *const RulesetAttr as libc::c_long,
        size as libc::c_long,
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

/// Grants `rights` beneath `path`, a directory or a single file. A path that
/// cannot be opened cannot be granted; refusing here keeps the policy honest
/// instead of silently narrowing it. The handle owns the descriptor, so no
/// exit path from here leaks it.
fn allow_path(ruleset: &Ruleset, path: &str, rights: u64) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open mount {}: {}", path, error))?;
    let is_dir = handle.metadata().map(|meta| meta.is_dir()).unwrap_or(false);
    let attr = PathBeneathAttr {
        allowed_access: if is_dir { rights } else { rights & FILE_RIGHTS },
        parent_fd: std::os::fd::AsRawFd::as_raw_fd(&handle),
    };
    let added = add_rule(ruleset.descriptor(), RULE_PATH_BENEATH, &attr as *const PathBeneathAttr as libc::c_long);
    if added != 0 {
        return Err(format!("cannot grant access beneath {}: {}", path, std::io::Error::last_os_error()));
    }
    Ok(())
}

fn allow_tcp_port(ruleset: &Ruleset, port: u16, access: u64) -> Result<(), String> {
    let attr = NetPortAttr { allowed_access: access, port: port as u64 };
    let added = add_rule(ruleset.descriptor(), RULE_NET_PORT, &attr as *const NetPortAttr as libc::c_long);
    if added != 0 {
        return Err(format!("cannot allow TCP port {}: {}", port, std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Listing a directory, and nothing beneath it.
const ACCESS_FS_READ_DIR: u64 = 1 << 3;

/// Reads everywhere except beneath `closed`. Landlock is an allow-list and a
/// grant covers everything under it, so the walk descends only along closed
/// paths: a subtree holding none is granted whole, a directory holding one is
/// granted listing only and its entries visited, a closed path is skipped. A
/// symlink is never granted: the kernel checks the path it resolves to, which
/// has its own place in the walk. An entry that cannot be opened, or appears
/// after the walk in a directory on a closed path, stays closed.
fn open_except(ruleset: &Ruleset, dir: &std::path::Path, closed: &[String]) -> Result<(), String> {
    let here = dir.to_string_lossy();
    let prefix = if here == "/" { "/".to_string() } else { format!("{here}/") };
    if closed.iter().any(|path| *path == here) {
        return Ok(());
    }
    if !closed.iter().any(|path| path.starts_with(&prefix)) {
        let _ = allow_path(ruleset, &here, READ_RIGHTS_ABI1);
        return Ok(());
    }
    allow_path(ruleset, &here, ACCESS_FS_READ_DIR)?;
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.flatten() {
        if entry.file_type().map(|kind| kind.is_symlink()).unwrap_or(true) {
            continue;
        }
        open_except(ruleset, &entry.path(), closed)?;
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
    let carve = !policy.restrict_reads && !policy.open_reads_except.is_empty();
    let reads = if policy.restrict_reads || carve { READ_RIGHTS_ABI1 } else { 0 };
    let handled_net = if policy.restrict_network { ACCESS_NET_CONNECT_TCP | ACCESS_NET_BIND_TCP } else { 0 };
    let ruleset = create_ruleset(abi, writes | reads, handled_net)?;
    for dir in &policy.writable_dirs {
        allow_path(&ruleset, dir, writes | reads)?;
    }
    if policy.restrict_reads {
        allow_reads(&ruleset, policy)?;
    } else if carve {
        open_except(&ruleset, std::path::Path::new("/"), &policy.open_reads_except)?;
    }
    for port in &policy.tcp_ports {
        allow_tcp_port(&ruleset, *port, ACCESS_NET_CONNECT_TCP)?;
    }
    for port in &policy.bind_ports {
        allow_tcp_port(&ruleset, *port, ACCESS_NET_BIND_TCP)?;
    }
    Ok(ruleset)
}

/// Reading is handled all-or-nothing: with reads restricted, a path no rule
/// names is closed, so the platform's own directories have to be named too.
fn allow_reads(ruleset: &Ruleset, policy: &Policy) -> Result<(), String> {
    let present = |path: &&String| std::path::Path::new(path.as_str()).exists();
    let system = policy.system_dirs.iter().chain(&policy.system_files).filter(present);
    for path in policy.readable_dirs.iter().chain(system) {
        allow_path(ruleset, path, READ_RIGHTS_ABI1)?;
    }
    Ok(())
}
