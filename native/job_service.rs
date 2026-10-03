//! Jobs: an operator's policy, a client's job, a worker process that runs it
//! in WASM, and the record of what happened. `porta job-check`, `job-run` and
//! `job-serve` all go through here; the Almide side binds to the functions at
//! the bottom through `wasmtime_bridge`.
//!
//! The boundary a job cannot cross is wasmtime's and WASI's: the module gets
//! fuel, a linear-memory ceiling, an epoch deadline, the directories it was
//! granted and nothing else. The process around it is a second, coarser
//! boundary: its own process group, an empty environment, a CPU-time and file
//! size ceiling, and a supervisor that kills it.

mod http;
mod policy;
mod spec;
mod supervisor;
mod worker;

use crate::locking::locked;
use crate::wasmtime_bridge::is_component;
use policy::{sha256_hex, valid_name, Access, Limits, LimitsFile, Policy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use spec::{Mount, Resolved};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use supervisor::Service;
use worker::WorkerResult;

static VERSION: OnceLock<String> = OnceLock::new();

fn util_version() -> String { VERSION.get().cloned().unwrap_or_default() }

/// `YYYYMMDDTHHMMSSZ-` and twelve random hex digits: sortable by start time,
/// unguessable enough that one client cannot name another's run by counting.
fn new_run_id() -> String {
    let mut random = [0u8; 6];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = file.read_exact(&mut random);
    }
    let stamp: String = rfc3339(SystemTime::now()).chars().filter(|c| c.is_ascii_digit() || *c == 'T').take(15).collect();
    format!("{stamp}Z-{}", random.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn valid_run_id(id: &str) -> bool {
    id.len() == 29 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// UTC, to the millisecond, without a date library.
fn rfc3339(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, millis) = (since.as_secs() as i64, since.subsec_millis());
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes).map_err(|e| format!("write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

fn failure(message: impl std::fmt::Display) -> String { json!({"error": message.to_string()}).to_string() }

/// `porta job-check`: the policy as a client would see it, and, given a job,
/// what it would be granted or why it would be refused. Nothing is created
/// beyond the policy's state directories and nothing runs.
pub fn job_check(policy: impl AsRef<str>, job: impl AsRef<str>) -> String {
    let policy = match Policy::load(Path::new(policy.as_ref())) { Ok(p) => p, Err(e) => return failure(e) };
    if job.as_ref().is_empty() { return json!({"policy": policy.summary()}).to_string(); }
    let body = match std::fs::read(job.as_ref()) { Ok(b) => b, Err(e) => return failure(format!("read job: {e}")) };
    match spec::resolve(&policy, &body) {
        Ok((job, digest)) => json!({"policy": policy.identity(), "accepted": true, "job_sha256": digest, "module": {"name": job.module, "sha256": job.module_sha256}, "limits": job.limits, "grants": job.grants()}).to_string(),
        Err(refusal) => json!({"policy": policy.identity(), "accepted": false, "code": refusal.code, "message": refusal.message}).to_string(),
    }
}

/// `porta job-run`: one job, through the same supervisor and worker the
/// service uses; returns the finished record.
pub fn job_run(version: impl AsRef<str>, policy: impl AsRef<str>, job: impl AsRef<str>) -> String {
    let _ = VERSION.set(version.as_ref().to_string());
    let service = match Policy::load(Path::new(policy.as_ref())).and_then(Service::open) { Ok(s) => s, Err(e) => return failure(e) };
    let body = match std::fs::read(job.as_ref()) { Ok(b) => b, Err(e) => return failure(format!("read job: {e}")) };
    match service.submit(&body) {
        Ok(record) => {
            let id = record["run_id"].as_str().unwrap_or_default().to_string();
            while service.is_live(&id) { let _ = service.record(&id, Duration::from_secs(1)); }
            service.record(&id, Duration::ZERO).map(|r| String::from_utf8_lossy(&pretty(&r)).trim_end().to_string()).unwrap_or_else(|| failure("the record is missing"))
        }
        Err(rejected) => String::from_utf8_lossy(&pretty(&rejected.record)).trim_end().to_string(),
    }
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_terminate(_: libc::c_int) { SHUTDOWN.store(true, Ordering::SeqCst); }

/// `porta job-serve`: the HTTP API until SIGTERM or SIGINT, which stop the
/// running jobs, finish their records as `service_shutdown`, and exit.
pub fn job_serve(version: impl AsRef<str>, policy: impl AsRef<str>, listen: impl AsRef<str>, no_auth: bool) -> String {
    let _ = VERSION.set(version.as_ref().to_string());
    let token = std::env::var("PORTA_JOB_TOKEN").ok().filter(|t| !t.is_empty());
    // The token stays in this process only; workers start from an empty
    // environment, and the variable is removed so nothing later inherits it.
    std::env::remove_var("PORTA_JOB_TOKEN");
    if let Err(reason) = authentication(token.as_deref(), no_auth, listen.as_ref()) { return failure(reason); }
    let service = match Policy::load(Path::new(policy.as_ref())).and_then(Service::open) { Ok(s) => s, Err(e) => return failure(e) };
    stop_on_signal(Arc::clone(&service));
    match http::serve(service, listen.as_ref(), token) { Ok(()) => String::new(), Err(e) => failure(e) }
}

/// A token, or no token on loopback when the operator says so in as many words.
fn authentication(token: Option<&str>, no_auth: bool, listen: &str) -> Result<(), &'static str> {
    let loopback = listen.parse::<std::net::SocketAddr>().map(|a| a.ip().is_loopback()).unwrap_or(false);
    match (token, no_auth) {
        (Some(t), _) if t.len() < 16 => Err("PORTA_JOB_TOKEN must be at least 16 characters"),
        (Some(_), true) => Err("--no-auth and PORTA_JOB_TOKEN together are ambiguous; choose one"),
        (None, false) => Err("set PORTA_JOB_TOKEN, or pass --no-auth to serve without one on a loopback address"),
        (None, true) if !loopback => Err("--no-auth is accepted only on a loopback address such as 127.0.0.1:8640"),
        _ => Ok(()),
    }
}

/// SIGTERM and SIGINT stop the running jobs, finish their records as
/// `service_shutdown`, and exit.
fn stop_on_signal(service: Arc<Service>) {
    // The handler only stores to an atomic, which is async-signal-safe.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        unsafe { libc::signal(signal, on_terminate as *const () as libc::sighandler_t); }
    }
    std::thread::spawn(move || {
        while !SHUTDOWN.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(100)); }
        eprintln!("[porta job-serve] stopping: running jobs end as service_shutdown");
        service.shutdown(Duration::from_secs(8));
        std::process::exit(0);
    });
}

/// `porta __job-worker <dir> <lifeline-fd>`: never run by hand; the
/// supervisor starts it.
pub fn job_worker(dir: impl AsRef<str>, lifeline: impl AsRef<str>) -> i64 {
    worker::main(Path::new(dir.as_ref()), lifeline.as_ref().parse().ok())
}
