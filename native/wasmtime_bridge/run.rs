//! Running one instance: its WASI view, its limits and its exit code.

use super::*;

/// Run _start. Returns exit code (0 = success, -1 = error/trap).
pub fn wt_run(handle: i64) -> i64 {
    let mut instances = locked(&INSTANCES);
    let Some(inst) = instances.get_mut(handle as usize).and_then(|slot| slot.as_mut()) else { return -1 };
    let stdout_pipe = MemoryOutputPipe::new(1024 * 1024);
    let stderr_pipe = MemoryOutputPipe::new(1024 * 1024);
    let ctx = PortaCtx {
        wasi: wasi_context(inst, &stdout_pipe, &stderr_pipe),
        limits: store_limits(inst.max_memory_bytes),
    };
    let mut store = Store::new(&inst.engine, ctx);
    store.limiter(|ctx| &mut ctx.limits);
    if inst.fuel > 0 {
        let _ = store.set_fuel(inst.fuel);
    }

    // Only WASI is linked. Direct porta.exec_command/http_request imports
    // bypassed MCP capability and allow-list checks; do not expose them.
    let mut linker = Linker::new(&inst.engine);
    if let Err(e) = p1::add_to_linker_sync(&mut linker, |ctx: &mut PortaCtx| &mut ctx.wasi) {
        return failed_run(inst, format!("linker setup failed: {}", e));
    }
    let instance = match linker.instantiate(&mut store, &inst.module) {
        Ok(i) => i,
        Err(e) => return failed_run(inst, format!("instantiation failed: {}", e)),
    };
    let entry = inst.entry_point.clone();
    let Some(result) = call_entry(&instance, &mut store, &entry) else {
        return failed_run(inst, format!("{} function not found", entry));
    };

    if inst.fuel > 0 {
        inst.fuel_consumed = inst.fuel.saturating_sub(store.get_fuel().unwrap_or(0));
    }
    // contents() clones, which avoids fighting the pipes over ref counts.
    inst.stdout_result = String::from_utf8_lossy(&stdout_pipe.contents()).to_string();
    inst.stderr_result = String::from_utf8_lossy(&stderr_pipe.contents()).to_string();
    drop(store);
    exit_status(inst, result)
}

/// Records why a run never reached its entry point, for the caller to read back.
fn failed_run(inst: &mut WasmInstance, reason: String) -> i64 {
    inst.stderr_result = reason;
    inst.exit_code = -1;
    -1
}

/// The guest's WASI view: its arguments, its explicit environment, a stdin that
/// is at end of input, and only the directories preopened for this instance.
fn wasi_context(inst: &WasmInstance, stdout: &MemoryOutputPipe, stderr: &MemoryOutputPipe) -> WasiP1Ctx {
    let mut wasi = WasiCtxBuilder::new();
    if !inst.wasi_args.is_empty() {
        wasi.args(&inst.wasi_args);
    }
    for (name, value) in &inst.env_vars {
        wasi.env(name, value);
    }
    // Empty stdin is immediate EOF, which is what a non-interactive run wants.
    wasi.stdin(MemoryInputPipe::new(inst.stdin_data.clone()));
    for (host, guest) in &inst.preopen_dirs {
        let _ = wasi.preopened_dir(
            host, guest,
            wasmtime_wasi::filesystem::DirPerms::all(),
            wasmtime_wasi::filesystem::FilePerms::all(),
        );
    }
    wasi.stdout(stdout.clone());
    wasi.stderr(stderr.clone());
    wasi.build_p1()
}

fn store_limits(max_memory_bytes: usize) -> StoreLimits {
    if max_memory_bytes > 0 {
        StoreLimitsBuilder::new().memory_size(max_memory_bytes).build()
    } else {
        StoreLimitsBuilder::new().build()
    }
}

/// Calls the entry point, which is `() -> ()` or, for a `main` returning a
/// result, `() -> i32`. A module exporting neither has nothing to run.
fn call_entry(instance: &Instance, store: &mut Store<PortaCtx>, entry: &str) -> Option<Result<(), Error>> {
    if let Ok(start) = instance.get_typed_func::<(), ()>(&mut *store, entry) {
        return Some(start.call(store, ()));
    }
    if let Ok(start) = instance.get_typed_func::<(), (i32,)>(&mut *store, entry) {
        return Some(start.call(store, ()).map(|_| ()));
    }
    None
}

/// The exit code a finished run reports.
fn exit_status(inst: &mut WasmInstance, result: Result<(), Error>) -> i64 {
    let Err(error) = result else {
        inst.exit_code = 0;
        return 0;
    };
    // proc_exit arrives as a trap but is a normal exit carrying its own code.
    if let Some(exit) = error.downcast_ref::<wasmtime_wasi::I32Exit>() {
        inst.exit_code = exit.0 as i64;
        return exit.0 as i64;
    }
    failed_run(inst, format!("{}", error))
}
