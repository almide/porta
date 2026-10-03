//! The worker: one process per job, started by the supervisor as
//! `porta __job-worker <dir>`. It reads the resolved job, compiles the module
//! from bytes whose digest it checks itself, runs it under fuel, a memory
//! ceiling and an epoch deadline with only WASI linked and only the granted
//! directories preopened, and writes what happened to `result.json`.
//!
//! No network is linked for a core module (WASI preview 1 has no outbound
//! sockets); a component gets WASI 0.2 with socket access and name lookup left
//! at their default, denied. No host function of porta's own is linked.

use super::*;
use wasmtime::{Config, Engine, Linker, Module, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, ResourceTable, I32Exit};
use wasmtime_wasi::filesystem::{DirPerms, FilePerms};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

/// How the guest ended, as the worker saw it.
#[derive(Serialize, Deserialize, Default, Debug)]
pub(super) struct WorkerResult {
    pub(super) stop_reason: String,
    pub(super) detail: String,
    pub(super) exit_code: Option<i32>,
    pub(super) fuel_consumed: u64,
    pub(super) memory_denied: bool,
    pub(super) peak_memory_bytes: u64,
}

/// A memory limiter that remembers whether it said no, so a trap that follows
/// a refused `memory.grow` is reported as the memory ceiling, not as a crash.
struct Tracked { inner: StoreLimits, denied: bool, peak: usize }

impl ResourceLimiter for Tracked {
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        let allowed = self.inner.memory_growing(current, desired, maximum)?;
        if allowed { self.peak = self.peak.max(desired) } else { self.denied = true }
        Ok(allowed)
    }
    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }
    fn instances(&self) -> usize { self.inner.instances() }
    fn tables(&self) -> usize { self.inner.tables() }
    fn memories(&self) -> usize { self.inner.memories() }
}

struct CoreCtx { wasi: wasmtime_wasi::p1::WasiP1Ctx, limits: Tracked }
struct ComponentCtx { wasi: WasiCtx, table: ResourceTable, limits: Tracked }
impl WasiView for ComponentCtx {
    fn ctx(&mut self) -> WasiCtxView<'_> { WasiCtxView { ctx: &mut self.wasi, table: &mut self.table } }
}

/// Entry point for `porta __job-worker <dir>`. The exit code only says whether
/// a result was written; the result itself says how the guest ended.
pub(super) fn main(dir: &Path, lifeline: Option<i32>) -> i64 {
    watch_parent(lifeline);
    let result = match std::fs::read(dir.join("job.json")).map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice::<Resolved>(&bytes).map_err(|e| e.to_string())) {
        Ok(job) => execute(dir, &job),
        Err(e) => WorkerResult { stop_reason: "worker_error".into(), detail: format!("read job: {e}"), ..Default::default() },
    };
    match write_atomic(&dir.join("result.json"), &serde_json::to_vec(&result).unwrap_or_default()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// A worker whose supervisor is gone has no one to report to and nothing to
/// stop it but its own deadline; it ends itself instead. The lifeline socket
/// reads end-of-file once the service has exited, whoever stands between the
/// two; where it did not arrive, a change of parent is the sign.
fn watch_parent(lifeline: Option<i32>) {
    use std::io::Read;
    use std::os::unix::io::FromRawFd;
    if let Some(fd) = lifeline {
        // The supervisor put this descriptor here for this purpose alone.
        let mut socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
        std::thread::spawn(move || {
            let mut byte = [0u8; 1];
            loop {
                match socket.read(&mut byte) {
                    Ok(0) => std::process::exit(137),
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    // Not a socket we were given: fall back to the parent check.
                    Err(_) => return,
                }
            }
        });
    }
    let parent = std::os::unix::process::parent_id();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        if std::os::unix::process::parent_id() != parent { std::process::exit(137); }
    });
}

/// What one run needs from the job, prepared once for either kind of module.
struct Setup<'a> { engine: Engine, bytes: &'a [u8], wasi: WasiCtxBuilder, limits: Tracked, fuel: u64 }

fn execute(dir: &Path, job: &Resolved) -> WorkerResult {
    let fail = |reason: &str, detail: String| WorkerResult { stop_reason: reason.into(), detail, ..Default::default() };
    // The bytes compiled are the bytes hashed: read once, check, compile those.
    let bytes = match std::fs::read(&job.module_path) { Ok(b) => b, Err(e) => return fail("module_unavailable", e.to_string()) };
    let actual = sha256_hex(&bytes);
    if actual != job.module_sha256 { return fail("module_changed", format!("module sha256 is {actual}, the policy pins {}", job.module_sha256)); }
    let cap = job.limits.max_output_bytes as usize;
    let stdout = MemoryOutputPipe::new(cap);
    let stderr = MemoryOutputPipe::new(cap.min(256 * 1024));
    let setup = match prepare(job, &bytes, &stdout, &stderr) { Ok(s) => s, Err(e) => return fail("worker_error", e) };
    let deadline = Deadline::start(&setup.engine, Duration::from_millis(job.limits.timeout_ms));
    let mut result = if is_component(&bytes) { run_component(setup) } else { run_module(setup) };
    drop(deadline);
    let _ = std::fs::write(dir.join("stdout"), stdout.contents());
    let _ = std::fs::write(dir.join("stderr"), stderr.contents());
    if result.stop_reason == "trap" && result.detail.contains("write beyond capacity") {
        result.stop_reason = "output_limit".into();
    }
    result
}

fn prepare<'a>(job: &Resolved, bytes: &'a [u8], stdout: &MemoryOutputPipe, stderr: &MemoryOutputPipe) -> Result<Setup<'a>, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(|e| e.to_string())?;
    let limits = Tracked {
        inner: StoreLimitsBuilder::new().memory_size((job.limits.memory_mib * 1024 * 1024) as usize).instances(64).tables(64).memories(16).table_elements(1_000_000).build(),
        denied: false,
        peak: 0,
    };
    Ok(Setup { engine, bytes, wasi: wasi_builder(job, stdout, stderr)?, limits, fuel: job.limits.fuel })
}

/// Bumps the engine's epoch once the wall-clock deadline passes, which traps
/// the guest at its next check; dropping it stops the timer.
struct Deadline { cancel: std::sync::mpsc::Sender<()>, timer: Option<std::thread::JoinHandle<()>> }

impl Deadline {
    fn start(engine: &Engine, after: Duration) -> Deadline {
        let engine = engine.clone();
        let (cancel, receiver) = std::sync::mpsc::channel::<()>();
        let timer = std::thread::spawn(move || {
            if matches!(receiver.recv_timeout(after), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) { engine.increment_epoch(); }
        });
        Deadline { cancel, timer: Some(timer) }
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(timer) = self.timer.take() { let _ = timer.join(); }
    }
}

fn wasi_builder(job: &Resolved, stdout: &MemoryOutputPipe, stderr: &MemoryOutputPipe) -> Result<WasiCtxBuilder, String> {
    let mut wasi = WasiCtxBuilder::new();
    let mut argv = vec![job.module.clone()];
    argv.extend(job.args.iter().cloned());
    wasi.args(&argv);
    for (name, value) in &job.env { wasi.env(name, value); }
    wasi.stdin(MemoryInputPipe::new(job.input.clone().into_bytes()));
    wasi.stdout(stdout.clone());
    wasi.stderr(stderr.clone());
    for mount in &job.mounts {
        let (dirs, files) = if mount.writable { (DirPerms::all(), FilePerms::all()) } else { (DirPerms::READ, FilePerms::READ) };
        wasi.preopened_dir(&mount.host, &mount.guest, dirs, files).map_err(|e| format!("preopen {}: {e}", mount.guest))?;
    }
    Ok(wasi)
}

fn run_module(setup: Setup) -> WorkerResult {
    let Setup { engine, bytes, mut wasi, limits, fuel } = setup;
    let engine = &engine;
    let mut store = Store::new(engine, CoreCtx { wasi: wasi.build_p1(), limits });
    store.limiter(|ctx| &mut ctx.limits);
    let _ = store.set_fuel(fuel);
    store.set_epoch_deadline(1);
    let module = match Module::from_binary(engine, bytes) { Ok(m) => m, Err(e) => return refused("invalid_module", &e) };
    let mut linker: Linker<CoreCtx> = Linker::new(engine);
    if let Err(e) = wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |ctx: &mut CoreCtx| &mut ctx.wasi) { return refused("worker_error", &e) }
    let instance = match linker.instantiate(&mut store, &module) { Ok(i) => i, Err(e) => return refused("link_refused", &e) };
    let outcome = match instance.get_typed_func::<(), ()>(&mut store, "_start") {
        Ok(start) => start.call(&mut store, ()),
        Err(_) => return WorkerResult { stop_reason: "invalid_module".into(), detail: "the module exports no _start".into(), ..Default::default() },
    };
    let left = store.get_fuel().unwrap_or(0);
    let ctx = store.data();
    finish(outcome, fuel.saturating_sub(left), &ctx.limits)
}

fn run_component(setup: Setup) -> WorkerResult {
    use wasmtime::component::{Component, Linker};
    use wasmtime_wasi::p2::bindings::sync::Command;
    let Setup { engine, bytes, mut wasi, limits, fuel } = setup;
    let engine = &engine;
    let mut store = Store::new(engine, ComponentCtx { wasi: wasi.build(), table: ResourceTable::new(), limits });
    store.limiter(|ctx| &mut ctx.limits);
    let _ = store.set_fuel(fuel);
    store.set_epoch_deadline(1);
    let component = match Component::from_binary(engine, bytes) { Ok(c) => c, Err(e) => return refused("invalid_module", &e) };
    let mut linker: Linker<ComponentCtx> = Linker::new(engine);
    if let Err(e) = wasmtime_wasi::p2::add_to_linker_sync(&mut linker) { return refused("worker_error", &e) }
    let command = match Command::instantiate(&mut store, &component, &linker) { Ok(c) => c, Err(e) => return refused("link_refused", &e) };
    let outcome = match command.wasi_cli_run().call_run(&mut store) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(())) => Err(wasmtime::Error::new(I32Exit(1))),
        Err(e) => Err(e),
    };
    let left = store.get_fuel().unwrap_or(0);
    finish(outcome, fuel.saturating_sub(left), &store.data().limits)
}

fn refused(reason: &str, error: &wasmtime::Error) -> WorkerResult {
    WorkerResult { stop_reason: reason.into(), detail: bounded(format!("{error:#}")), ..Default::default() }
}

/// Names the reason a run ended. A trap after a refused memory grow is the
/// memory ceiling; fuel and the epoch deadline have their own trap codes.
fn finish(outcome: wasmtime::Result<()>, fuel_consumed: u64, limits: &Tracked) -> WorkerResult {
    let mut result = WorkerResult { fuel_consumed, memory_denied: limits.denied, peak_memory_bytes: limits.peak as u64, ..Default::default() };
    match outcome {
        Ok(()) => { result.stop_reason = "exit".into(); result.exit_code = Some(0); }
        Err(error) => {
            if let Some(exit) = error.downcast_ref::<I32Exit>() {
                result.exit_code = Some(exit.0);
                result.stop_reason = if exit.0 == 0 { "exit" } else if limits.denied { "memory_limit" } else { "exit_nonzero" }.into();
            } else {
                result.stop_reason = match error.downcast_ref::<Trap>() {
                    Some(Trap::OutOfFuel) => "fuel_exhausted",
                    Some(Trap::Interrupt) => "timeout",
                    _ if limits.denied => "memory_limit",
                    _ => "trap",
                }.into();
            }
            result.detail = bounded(format!("{error:#}"));
        }
    }
    result
}

/// A guest controls its trap messages through its names and its output; keep
/// what reaches the record short.
fn bounded(text: String) -> String {
    if text.len() <= 2048 { return text; }
    let mut end = 2048;
    while !text.is_char_boundary(end) { end -= 1; }
    format!("{}…", &text[..end])
}
