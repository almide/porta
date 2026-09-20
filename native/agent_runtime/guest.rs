//! Running one guest module: its store, its limits, and the wall-clock
//! deadline that interrupts it.
//!
//! An instance inherits no host environment and no implicit directory. It
//! sees the mounts it was granted and nothing else.

use super::*;

// A cancellable timer interrupts CPU-bound WASM at the wall-clock deadline.
// Drop always joins it, including traps and instantiation errors.
struct Deadline {
    cancel: std::sync::mpsc::Sender<()>,
    timer: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(timer) = self.timer.take() { let _ = timer.join(); }
    }
}

impl Runtime {
    pub(super) fn execute(&self, guest: &Guest, input: &[u8]) -> Result<(String, u64), String> {
        self.remaining()?;
        if input.len() > MAX_MESSAGE { return Err("guest input exceeds 1 MiB".into()); }
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdin(MemoryInputPipe::new(input.to_vec()));
        let stdout = MemoryOutputPipe::new(MAX_MESSAGE);
        let stderr = MemoryOutputPipe::new(64 * 1024);
        wasi.stdout(stdout.clone()).stderr(stderr);
        for mount in &guest.mounts {
            let (dirs, files) = if mount.read_only { (DirPerms::READ, FilePerms::READ) } else { (DirPerms::all(), FilePerms::all()) };
            wasi.preopened_dir(&mount.host, &mount.guest, dirs, files).map_err(|e| format!("preopen tool directory: {e}"))?;
        }
        let limits = StoreLimitsBuilder::new().memory_size(self.config.limits.memory_pages * 65536).memories(1).tables(4).table_elements(100_000).build();
        let mut store = Store::new(&self.engine, Context { wasi:wasi.build_p1(), limits });
        store.limiter(|ctx| &mut ctx.limits);
        store.set_fuel(self.config.limits.fuel_per_step).map_err(|e| e.to_string())?;
        store.set_epoch_deadline(1);
        let engine = self.engine.clone();
        let remaining = self.remaining()?;
        let (cancel, receiver) = std::sync::mpsc::channel();
        let _deadline = Deadline { cancel, timer: Some(std::thread::spawn(move || {
            if matches!(receiver.recv_timeout(remaining), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                engine.increment_epoch();
            }
        })) };
        let instance = guest.prepared.instantiate(&mut store).map_err(|e| format!("instantiate guest: {e}"))?;
        let start = instance.get_typed_func::<(), ()>(&mut store, "_start").map_err(|e| format!("guest _start: {e}"))?;
        if let Err(e) = start.call(&mut store, ()) {
            if !e.downcast_ref::<wasmtime_wasi::I32Exit>().is_some_and(|e| e.0 == 0) { return Err(format!("guest execution failed: {e:#}")); }
        }
        self.remaining()?;
        let fuel = self.config.limits.fuel_per_step - store.get_fuel().unwrap_or(0);
        let output = String::from_utf8(stdout.contents().to_vec()).map_err(|_| "guest output must be UTF-8")?;
        Ok((output, fuel))
    }
}
