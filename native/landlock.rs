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
    pub tcp_ports: Vec<u16>,
    pub restrict_network: bool,
}

/// A prepared ruleset. Dropping it closes the descriptor.
pub struct Ruleset {
    fd: i32,
}

impl Drop for Ruleset {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

impl Ruleset {
    pub fn descriptor(&self) -> i32 {
        self.fd
    }

    /// Applies the ruleset to the calling process. Safe to call after fork:
    /// two syscalls, no allocation, no locks.
    pub fn restrict_current_process(fd: i32) -> std::io::Result<()> {
        let no_new_privs = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if no_new_privs != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let restricted = unsafe { libc::syscall(SYS_RESTRICT_SELF, fd, 0) };
        if restricted != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Landlock ABI the running kernel reports, or an error when it has none.
pub fn abi_version() -> Result<i64, String> {
    let abi = unsafe {
        libc::syscall(
            SYS_CREATE_RULESET,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            CREATE_RULESET_VERSION,
        )
    };
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
    let fd = unsafe {
        libc::syscall(
            SYS_CREATE_RULESET,
            &attr as *const RulesetAttr,
            std::mem::size_of::<RulesetAttr>(),
            0,
        )
    };
    if fd < 0 {
        return Err(format!("Landlock ruleset refused: {}", std::io::Error::last_os_error()));
    }
    Ok(Ruleset { fd: fd as i32 })
}

fn allow_directory(ruleset: &Ruleset, dir: &str, rights: u64) -> Result<(), String> {
    let path = match std::ffi::CString::new(dir) {
        Ok(path) => path,
        Err(_) => return Err(format!("mount path contains a NUL byte: {}", dir)),
    };
    let dir_fd = unsafe { libc::open(path.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if dir_fd < 0 {
        // A mount that cannot be opened cannot be granted. Refusing here keeps
        // the policy honest instead of silently narrowing it.
        return Err(format!("cannot open mount {}: {}", dir, std::io::Error::last_os_error()));
    }
    let attr = PathBeneathAttr { allowed_access: rights, parent_fd: dir_fd };
    let added = unsafe {
        libc::syscall(SYS_ADD_RULE, ruleset.fd, RULE_PATH_BENEATH, &attr as *const PathBeneathAttr, 0)
    };
    unsafe { libc::close(dir_fd) };
    if added != 0 {
        return Err(format!("cannot grant writes beneath {}: {}", dir, std::io::Error::last_os_error()));
    }
    Ok(())
}

fn allow_tcp_port(ruleset: &Ruleset, port: u16) -> Result<(), String> {
    let attr = NetPortAttr { allowed_access: ACCESS_NET_CONNECT_TCP, port: port as u64 };
    let added = unsafe {
        libc::syscall(SYS_ADD_RULE, ruleset.fd, RULE_NET_PORT, &attr as *const NetPortAttr, 0)
    };
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
    let rights = write_rights(abi);
    let handled_net = if policy.restrict_network { ACCESS_NET_CONNECT_TCP } else { 0 };
    let ruleset = create_ruleset(rights, handled_net)?;
    for dir in &policy.writable_dirs {
        allow_directory(&ruleset, dir, rights)?;
    }
    for port in &policy.tcp_ports {
        allow_tcp_port(&ruleset, *port)?;
    }
    Ok(ruleset)
}
