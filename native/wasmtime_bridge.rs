// Wasmtime bridge: Rust module callable from Almide via @extern(rs).
// Provides a handle-based API for WASM instance lifecycle management.

use crate::json_text::escape_json_text;
use crate::locking::locked;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::io::{Read, Write, BufRead, BufReader};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wasmtime::*;
#[allow(unused_imports)]
use serde_json;
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, ResourceTable};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

mod load;
use load::{engine_config, load_code, Code, ComponentCtx};
pub(crate) use load::is_component;

struct WasmInstance {
    engine: Engine,
    code: Code,
    stdin_data: Vec<u8>,
    wasi_args: Vec<String>,
    env_vars: Vec<(String, String)>,
    preopen_dirs: Vec<(String, String)>,
    fuel: u64,
    max_memory_bytes: usize,
    entry_point: String,
    stdout_result: String,
    stderr_result: String,
    exit_code: i64,
    fuel_consumed: u64,
}

struct PortaCtx {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

static INSTANCES: Mutex<Vec<Option<WasmInstance>>> = Mutex::new(Vec::new());

// The Almide side resolves these by module, so the FFI surface stays here while
// the implementations live next to the state they own.
pub use crate::http_proxy::{wt_is_host_allowed, wt_proxy_start, wt_proxy_stop};
pub use crate::sandbox_exec::{wt_exec_inner, wt_exec_replace, wt_exec_sandboxed, wt_exec_supervised, wt_parse_toml, wt_sandbox_explain, wt_sandbox_explain_json};
pub use crate::sandbox_check::{wt_sandbox_check, wt_sandbox_check_json};
pub use crate::sandbox_profile::wt_sandbox_profile;
pub use crate::http_client::wt_http_request;
pub use crate::host_process::{wt_exec_command, wt_getpid, wt_home_dir, wt_kill, wt_spawn};
pub use crate::wasm_inspect::wt_inspect;

mod run;
pub use run::wt_run;

/// Create a WASM instance from a file path.
/// Returns handle (>= 0) on success, -1 on error.
pub fn wt_create(wasm_path: impl AsRef<str>, fuel: i64) -> i64 {
    let path = wasm_path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return -1,
    };

    let Ok(engine) = Engine::new(&engine_config(fuel)) else { return -1 };
    let Some(code) = load_code(&engine, &bytes) else { return -1 };

    let inst = WasmInstance {
        engine,
        code,
        stdin_data: Vec::new(),
        wasi_args: Vec::new(),
        env_vars: Vec::new(),
        preopen_dirs: Vec::new(),
        fuel: if fuel > 0 { fuel as u64 } else { 0 },
        max_memory_bytes: 0,
        entry_point: "_start".to_string(),
        stdout_result: String::new(),
        stderr_result: String::new(),
        exit_code: 0,
        fuel_consumed: 0,
    };

    let mut instances = locked(&INSTANCES);
    let handle = instances.len() as i64;
    instances.push(Some(inst));
    handle
}

/// Set stdin data for an instance (must be called before wt_run).
pub fn wt_set_stdin(handle: i64, data: impl AsRef<str>) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data.as_ref().as_bytes().to_vec(); 0 }
        None => -1,
    }
}

/// Set stdin data as raw bytes for an instance.
pub fn wt_set_stdin_bytes(handle: i64, data: Vec<u8>) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data; 0 }
        None => -1,
    }
}

/// Set stdin as length-prefixed JSON tool command (for MCP tool dispatch).
pub fn wt_set_tool_stdin(handle: i64, tool_name: impl AsRef<str>, args_json: impl AsRef<str>) -> i64 {
    let cmd = format!("{{\"tool\":\"{}\",\"arguments\":{}}}", tool_name.as_ref(), args_json.as_ref());
    let cmd_bytes = cmd.as_bytes();
    let len = cmd_bytes.len() as u32;
    let mut data = Vec::with_capacity(4 + cmd_bytes.len());
    data.push((len & 0xFF) as u8);
    data.push(((len >> 8) & 0xFF) as u8);
    data.push(((len >> 16) & 0xFF) as u8);
    data.push(((len >> 24) & 0xFF) as u8);
    data.extend_from_slice(cmd_bytes);

    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data; 0 }
        None => -1,
    }
}

/// Set WASI command-line arguments (must be called before wt_run).
/// args_json: JSON array of strings, e.g. ["python.wasm", "script.py"]
pub fn wt_set_args(handle: i64, args_json: impl AsRef<str>) -> i64 {
    let args: Vec<String> = match serde_json::from_str(args_json.as_ref()) {
        Ok(a) => a,
        Err(_) => return -1,
    };
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.wasi_args = args; 0 }
        None => -1,
    }
}

/// Add an environment variable (must be called before wt_run).
pub fn wt_set_env(handle: i64, key: impl AsRef<str>, value: impl AsRef<str>) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.env_vars.push((key.as_ref().to_string(), value.as_ref().to_string())); 0 }
        None => -1,
    }
}

/// Set maximum memory in WASM pages (64KB each). 0 = unlimited.
pub fn wt_set_max_memory(handle: i64, pages: i64) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => {
            inst.max_memory_bytes = if pages > 0 { pages as usize * 65536 } else { 0 };
            0
        }
        None => -1,
    }
}

/// Set entry point function name (default: "_start"). Must be called before wt_run.
pub fn wt_set_entry(handle: i64, name: impl AsRef<str>) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.entry_point = name.as_ref().to_string(); 0 }
        None => -1,
    }
}

/// Add a preopened directory (must be called before wt_run).
/// host_path: actual path on the host filesystem.
/// guest_path: path the WASM agent sees (e.g., "." or "/work").
pub fn wt_preopen_dir(handle: i64, host_path: impl AsRef<str>, guest_path: impl AsRef<str>) -> i64 {
    let mut instances = locked(&INSTANCES);
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => {
            inst.preopen_dirs.push((host_path.as_ref().to_string(), guest_path.as_ref().to_string()));
            0
        }
        None => -1,
    }
}


/// Get captured stdout after wt_run.
pub fn wt_get_stdout(handle: i64) -> String {
    let instances = locked(&INSTANCES);
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.stdout_result.clone())
        .unwrap_or_default()
}

/// Get captured stderr after wt_run.
pub fn wt_get_stderr(handle: i64) -> String {
    let instances = locked(&INSTANCES);
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.stderr_result.clone())
        .unwrap_or_default()
}

/// Get fuel consumed (steps executed) after wt_run.
pub fn wt_get_fuel_consumed(handle: i64) -> i64 {
    let instances = locked(&INSTANCES);
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.fuel_consumed as i64)
        .unwrap_or(0)
}

/// Get exit code after wt_run.
pub fn wt_get_exit_code(handle: i64) -> i64 {
    let instances = locked(&INSTANCES);
    instances.get(handle as usize)
        .and_then(|s| s.as_ref())
        .map(|i| i.exit_code)
        .unwrap_or(-1)
}





/// Destroy an instance and free resources.
pub fn wt_destroy(handle: i64) -> i64 {
    let mut instances = locked(&INSTANCES);
    let idx = handle as usize;
    if idx < instances.len() {
        instances[idx] = None;
        0
    } else {
        -1
    }
}


