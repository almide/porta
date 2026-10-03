//! Getting a worker running: its workspace, its command line, the thread that
//! forks it, and whether the OS sandbox around it can be applied here.

use super::*;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// Time the supervisor allows past the job's own deadline — for compiling the
/// module and writing the result — before it kills the worker itself.
pub(super) const GRACE: Duration = Duration::from_secs(5);

type Request = (Command, std::sync::mpsc::Sender<std::io::Result<Child>>);

/// Starts workers from one thread that lives as long as the service. On Linux
/// a child's `PR_SET_PDEATHSIG` fires when the *thread* that forked it ends,
/// not the process; a worker forked from a short-lived request thread would
/// be killed as soon as that request was answered.
///
/// It also holds the lifeline: a socket pair whose one end only this process
/// keeps (close-on-exec) and whose other end every worker inherits. When the
/// service dies, however it dies, the workers read end-of-file and stop —
/// including under `porta run`, where a worker's parent is not the service.
pub(super) struct Spawner {
    requests: Mutex<std::sync::mpsc::Sender<Request>>,
    _held: UnixStream,
    inherited: UnixStream,
}

impl Spawner {
    pub(super) fn start() -> Result<Spawner, String> {
        let (held, inherited) = UnixStream::pair().map_err(|e| format!("create the worker lifeline: {e}"))?;
        let (requests, incoming) = std::sync::mpsc::channel::<Request>();
        std::thread::spawn(move || {
            for (mut command, reply) in incoming { let _ = reply.send(command.spawn()); }
        });
        Ok(Spawner { requests: Mutex::new(requests), _held: held, inherited })
    }

    pub(super) fn lifeline(&self) -> i32 { self.inherited.as_raw_fd() }

    pub(super) fn spawn(&self, command: Command) -> Result<Child, String> {
        let stopped = || "the worker spawner has stopped".to_string();
        let (reply, answer) = std::sync::mpsc::channel();
        locked(&self.requests).send((command, reply)).map_err(|_| stopped())?;
        answer.recv().map_err(|_| stopped())?.map_err(|e| format!("start worker: {e}"))
    }
}

/// The workspace: `input/` written from the job and mounted read-only,
/// `output/` empty and writable, and the job the worker will read. Granted
/// directories are mounted at `/data/<name>`.
pub(super) fn prepare(workspace: &Path, job: &mut Resolved, policy: &Policy) -> Result<(), String> {
    let input = workspace.join("input");
    let output = workspace.join("output");
    for dir in [workspace, &input, &output] {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(workspace, std::fs::Permissions::from_mode(0o700));
    }
    for (name, content) in &job.files {
        std::fs::write(input.join(name), content).map_err(|e| format!("write input {name}: {e}"))?;
    }
    job.mounts.push(Mount { host: input, guest: "/input".into(), writable: false });
    if job.output { job.mounts.push(Mount { host: output, guest: "/output".into(), writable: true }); }
    for (name, access) in &job.directories {
        let granted = &policy.directories[name];
        job.mounts.push(Mount { host: granted.host.clone(), guest: format!("/data/{name}"), writable: *access == Access::ReadWrite });
    }
    write_atomic(&workspace.join("job.json"), &serde_json::to_vec(job).map_err(|e| e.to_string())?)
}

/// `porta __job-worker <workspace> <lifeline>` with an empty environment, no
/// stdin, its own process group, no core dumps, a CPU-time backstop past the
/// deadline and a largest-file ceiling at the output limit; on Linux it also
/// dies with the spawner thread, which lives as long as the service.
///
/// With the OS sandbox on, the worker runs under `porta run` itself: writes
/// only to its workspace and to the directories the job was granted
/// read-write, reads confined to those, the module's directory and porta's
/// own, and no network. That boundary does not depend on wasmtime being right.
pub(super) fn worker_command(workspace: &Path, job: &Resolved, os_sandbox: bool, lifeline: i32) -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("locate porta: {e}"))?;
    let log = std::fs::File::create(workspace.join("worker.log")).map_err(|e| format!("worker log: {e}"))?;
    let mut command = Command::new(&exe);
    command.env_clear();
    if os_sandbox {
        command.args(sandbox_arguments(&exe, workspace, job)?);
        pass_home_and_path(&mut command);
    }
    command.arg("__job-worker").arg(workspace).arg(lifeline.to_string()).current_dir(workspace)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    let ceilings = [
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_CPU, job.limits.timeout_ms / 1000 + GRACE.as_secs() + 1),
        (libc::RLIMIT_FSIZE, job.limits.max_output_bytes + 1024 * 1024),
    ];
    use std::os::unix::process::CommandExt;
    // Between fork and exec: only async-signal-safe calls.
    unsafe {
        command.pre_exec(move || {
            libc::setpgid(0, 0);
            for (resource, value) in ceilings {
                let limit = libc::rlimit { rlim_cur: value as libc::rlim_t, rlim_max: value as libc::rlim_t };
                libc::setrlimit(resource, &limit);
            }
            // The lifeline is close-on-exec in the service; the worker keeps it.
            libc::fcntl(lifeline, libc::F_SETFD, 0);
            #[cfg(target_os = "linux")]
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    Ok(command)
}

/// `porta run` resolves the preset's `~/` paths and finds its tools with
/// these; the guest never sees them, its environment is the job's.
fn pass_home_and_path(command: &mut Command) {
    for name in ["PATH", "HOME"] {
        if let Ok(value) = std::env::var(name) { command.env(name, value); }
    }
}

fn text(path: &Path) -> Result<String, String> {
    path.to_str().map(String::from).ok_or_else(|| format!("{} is not UTF-8", path.display()))
}

/// `porta run <porta> -v ... --no-net --read-policy strict --`, to which the
/// caller appends the worker's own arguments.
fn sandbox_arguments(exe: &Path, workspace: &Path, job: &Resolved) -> Result<Vec<String>, String> {
    let read_only = |path: &Path| text(path).map(|p| format!("{p}:ro"));
    let mut args = vec!["run".to_string(), text(exe)?, "-v".into(), text(workspace)?];
    // Under strict reads porta has to be able to read itself to start.
    args.extend(["-v".into(), read_only(exe.parent().ok_or("porta has no directory")?)?]);
    args.extend(["-v".into(), read_only(job.module_path.parent().ok_or("the module has no directory")?)?]);
    for mount in job.mounts.iter().filter(|m| !m.host.starts_with(workspace)) {
        args.extend(["-v".into(), if mount.writable { text(&mount.host)? } else { read_only(&mount.host)? }]);
    }
    args.extend(["--no-net", "--read-policy", "strict", "--"].map(String::from));
    Ok(args)
}

/// Whether `porta run` can apply the worker's sandbox on this host, tried
/// once at start with the same flags a worker gets.
pub(super) fn probe_os_sandbox(workspaces: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("locate porta: {e}"))?;
    let install = format!("{}:ro", text(exe.parent().unwrap_or(Path::new("/")))?);
    let mut command = Command::new(&exe);
    command.args(["run", &text(&exe)?, "-v", &text(workspaces)?, "-v", &install, "--no-net", "--read-policy", "strict", "--", "--version"])
        .env_clear().stdin(Stdio::null());
    pass_home_and_path(&mut command);
    let output = command.output().map_err(|e| format!("start porta run: {e}"))?;
    if output.status.success() && String::from_utf8_lossy(&output.stdout).starts_with("porta ") { return Ok(()); }
    let said = String::from_utf8_lossy(&output.stderr);
    Err(format!("service.os_sandbox is \"required\" but this host cannot apply it: {}. Set service.os_sandbox = \"off\" to run jobs with the WASM boundary alone.",
        said.trim().lines().last().unwrap_or("porta run failed")))
}

pub(super) fn kill_group(pid: i32) {
    if pid > 0 { unsafe { libc::kill(-pid, libc::SIGKILL); } }
}
