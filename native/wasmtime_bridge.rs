// Wasmtime bridge: Rust module callable from Almide via @extern(rs).
// Provides a handle-based API for WASM instance lifecycle management.

use std::sync::Mutex;
use wasmtime::*;
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

struct WasmInstance {
    engine: Engine,
    module: Module,
    stdin_data: Vec<u8>,
    env_vars: Vec<(String, String)>,
    fuel: u64,
    stdout_result: String,
    stderr_result: String,
    exit_code: i64,
    fuel_consumed: u64,
}

static INSTANCES: Mutex<Vec<Option<WasmInstance>>> = Mutex::new(Vec::new());

/// Create a WASM instance from a file path.
/// Returns handle (>= 0) on success, -1 on error.
pub fn wt_create(wasm_path: &str, fuel: i64) -> i64 {
    let bytes = match std::fs::read(wasm_path) {
        Ok(b) => b,
        Err(_) => return -1,
    };
    wt_create_from_bytes(bytes, fuel)
}

/// Create a WASM instance from raw bytes.
/// Returns handle (>= 0) on success, -1 on error.
fn wt_create_from_bytes(wasm_bytes: Vec<u8>, fuel: i64) -> i64 {
    let mut config = Config::new();
    if fuel > 0 {
        config.consume_fuel(true);
    }
    config.wasm_multi_memory(true);

    let engine = match Engine::new(&config) {
        Ok(e) => e,
        Err(_) => return -1,
    };
    let module = match Module::from_binary(&engine, &wasm_bytes) {
        Ok(m) => m,
        Err(_) => return -1,
    };

    let inst = WasmInstance {
        engine,
        module,
        stdin_data: Vec::new(),
        env_vars: Vec::new(),
        fuel: if fuel > 0 { fuel as u64 } else { 0 },
        stdout_result: String::new(),
        stderr_result: String::new(),
        exit_code: 0,
        fuel_consumed: 0,
    };

    let mut instances = INSTANCES.lock().unwrap();
    let handle = instances.len() as i64;
    instances.push(Some(inst));
    handle
}

/// Set stdin data for an instance (must be called before wt_run).
pub fn wt_set_stdin(handle: i64, data: &str) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data.as_bytes().to_vec(); 0 }
        None => -1,
    }
}

/// Set stdin data as raw bytes for an instance.
pub fn wt_set_stdin_bytes(handle: i64, data: Vec<u8>) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data; 0 }
        None => -1,
    }
}

/// Add an environment variable (must be called before wt_run).
pub fn wt_set_env(handle: i64, key: &str, value: &str) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.env_vars.push((key.to_string(), value.to_string())); 0 }
        None => -1,
    }
}

/// Run _start. Returns exit code (0 = success, -1 = error/trap).
pub fn wt_run(handle: i64) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    let inst = match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(i) => i,
        None => return -1,
    };

    // Build WASI context
    let mut wasi = WasiCtxBuilder::new();
    for (k, v) in &inst.env_vars {
        wasi.env(k, v);
    }
    if !inst.stdin_data.is_empty() {
        wasi.stdin(MemoryInputPipe::new(inst.stdin_data.clone()));
    }
    let stdout_pipe = MemoryOutputPipe::new(1024 * 1024);
    let stderr_pipe = MemoryOutputPipe::new(1024 * 1024);
    wasi.stdout(stdout_pipe.clone());
    wasi.stderr(stderr_pipe.clone());

    let wasi_ctx = wasi.build_p1();
    let mut store = Store::new(&inst.engine, wasi_ctx);

    if inst.fuel > 0 {
        let _ = store.set_fuel(inst.fuel);
    }

    let mut linker = Linker::new(&inst.engine);
    if p1::add_to_linker_sync(&mut linker, |ctx| ctx).is_err() {
        return -1;
    }

    let instance = match linker.instantiate(&mut store, &inst.module) {
        Ok(i) => i,
        Err(_) => return -1,
    };

    let start = match instance.get_typed_func::<(), ()>(&mut store, "_start") {
        Ok(f) => f,
        Err(_) => return -1,
    };

    let result = start.call(&mut store, ());

    // Read fuel consumed
    if inst.fuel > 0 {
        let remaining = store.get_fuel().unwrap_or(0);
        inst.fuel_consumed = inst.fuel.saturating_sub(remaining);
    }

    // Capture stdout/stderr
    drop(store);
    inst.stdout_result = String::from_utf8(stdout_pipe.try_into_inner().unwrap_or_default().to_vec()).unwrap_or_default();
    inst.stderr_result = String::from_utf8(stderr_pipe.try_into_inner().unwrap_or_default().to_vec()).unwrap_or_default();

    match result {
        Ok(()) => { inst.exit_code = 0; 0 }
        Err(e) => {
            // Check for proc_exit (normal exit with code)
            if let Some(exit) = e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                inst.exit_code = exit.0 as i64;
                exit.0 as i64
            } else {
                inst.exit_code = -1;
                inst.stderr_result = format!("{}", e);
                -1
            }
        }
    }
}

/// Get captured stdout after wt_run.
pub fn wt_get_stdout(handle: i64) -> String {
    let instances = INSTANCES.lock().unwrap();
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.stdout_result.clone())
        .unwrap_or_default()
}

/// Get captured stderr after wt_run.
pub fn wt_get_stderr(handle: i64) -> String {
    let instances = INSTANCES.lock().unwrap();
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.stderr_result.clone())
        .unwrap_or_default()
}

/// Get fuel consumed (steps executed) after wt_run.
pub fn wt_get_fuel_consumed(handle: i64) -> i64 {
    let instances = INSTANCES.lock().unwrap();
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.fuel_consumed as i64)
        .unwrap_or(0)
}

/// Get exit code after wt_run.
pub fn wt_get_exit_code(handle: i64) -> i64 {
    let instances = INSTANCES.lock().unwrap();
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.exit_code)
        .unwrap_or(-1)
}

/// Destroy an instance and free resources.
pub fn wt_destroy(handle: i64) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    let idx = handle as usize;
    if idx < instances.len() {
        instances[idx] = None;
        0
    } else {
        -1
    }
}
