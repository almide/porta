// Wasmtime bridge: Rust module callable from Almide via @extern(rs).
// Provides a handle-based API for WASM instance lifecycle management.

use std::sync::Mutex;
use wasmtime::*;
#[allow(unused_imports)]
use serde_json;
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

struct WasmInstance {
    engine: Engine,
    module: Module,
    stdin_data: Vec<u8>,
    env_vars: Vec<(String, String)>,
    preopen_dirs: Vec<(String, String)>,
    fuel: u64,
    stdout_result: String,
    stderr_result: String,
    exit_code: i64,
    fuel_consumed: u64,
}

static INSTANCES: Mutex<Vec<Option<WasmInstance>>> = Mutex::new(Vec::new());

/// Create a WASM instance from a file path.
/// Returns handle (>= 0) on success, -1 on error.
pub fn wt_create(wasm_path: impl AsRef<str>, fuel: i64) -> i64 {
    let bytes = match std::fs::read(wasm_path.as_ref()) {
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
        preopen_dirs: Vec::new(),
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
pub fn wt_set_stdin(handle: i64, data: impl AsRef<str>) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data.as_ref().as_bytes().to_vec(); 0 }
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

    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.stdin_data = data; 0 }
        None => -1,
    }
}

/// Add an environment variable (must be called before wt_run).
pub fn wt_set_env(handle: i64, key: impl AsRef<str>, value: impl AsRef<str>) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.env_vars.push((key.as_ref().to_string(), value.as_ref().to_string())); 0 }
        None => -1,
    }
}

/// Add a preopened directory (must be called before wt_run).
/// host_path: actual path on the host filesystem.
/// guest_path: path the WASM agent sees (e.g., "." or "/work").
pub fn wt_preopen_dir(handle: i64, host_path: impl AsRef<str>, guest_path: impl AsRef<str>) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => {
            inst.preopen_dirs.push((host_path.as_ref().to_string(), guest_path.as_ref().to_string()));
            0
        }
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
    for (host, guest) in &inst.preopen_dirs {
        let _ = wasi.preopened_dir(
            host, guest,
            wasmtime_wasi::filesystem::DirPerms::all(),
            wasmtime_wasi::filesystem::FilePerms::all(),
        );
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
    if let Err(e) = p1::add_to_linker_sync(&mut linker, |ctx| ctx) {
        inst.stderr_result = format!("linker setup failed: {}", e);
        inst.exit_code = -1;
        return -1;
    }

    let instance = match linker.instantiate(&mut store, &inst.module) {
        Ok(i) => i,
        Err(e) => {
            inst.stderr_result = format!("instantiation failed: {}", e);
            inst.exit_code = -1;
            return -1;
        }
    };

    // Try () -> () first, then () -> i32 (for effect fn main returning Result)
    let result = if let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, "_start") {
        start.call(&mut store, ())
    } else if let Ok(start) = instance.get_typed_func::<(), (i32,)>(&mut store, "_start") {
        start.call(&mut store, ()).map(|_| ())
    } else {
        inst.stderr_result = "_start function not found".to_string();
        inst.exit_code = -1;
        return -1;
    };

    // Read fuel consumed
    if inst.fuel > 0 {
        let remaining = store.get_fuel().unwrap_or(0);
        inst.fuel_consumed = inst.fuel.saturating_sub(remaining);
    }

    // Capture stdout/stderr (use contents() which clones, avoiding Arc ref count issues)
    inst.stdout_result = String::from_utf8_lossy(&stdout_pipe.contents()).to_string();
    inst.stderr_result = String::from_utf8_lossy(&stderr_pipe.contents()).to_string();
    drop(store);

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

// --- HTTP host function ---

/// Execute an HTTP request. Returns JSON response string.
/// Response: {"status":200,"body":"..."} or {"error":"..."}
pub fn wt_http_request(method: impl AsRef<str>, url: impl AsRef<str>, headers_json: impl AsRef<str>, body: impl AsRef<str>) -> String {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build() {
        Ok(c) => c,
        Err(e) => return format!("{{\"error\":\"client error: {}\"}}", e),
    };

    let mut req = match method.as_ref() {
        "GET" => client.get(url.as_ref()),
        "POST" => client.post(url.as_ref()),
        "PUT" => client.put(url.as_ref()),
        "DELETE" => client.delete(url.as_ref()),
        "PATCH" => client.patch(url.as_ref()),
        "HEAD" => client.head(url.as_ref()),
        _ => return format!("{{\"error\":\"unsupported method: {}\"}}", method.as_ref()),
    };

    // Parse headers JSON: {"Content-Type": "application/json", ...}
    if !headers_json.as_ref().is_empty() && headers_json.as_ref() != "{}" {
        if let Ok(headers) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(headers_json.as_ref()) {
            for (k, v) in headers {
                if let Some(s) = v.as_str() {
                    req = req.header(k.as_str(), s);
                }
            }
        }
    }

    let body_str = body.as_ref();
    if !body_str.is_empty() {
        req = req.body(body_str.to_string());
    }

    match req.send() {
        Ok(resp) => {
            let status = resp.status().as_u16();
            match resp.text() {
                Ok(text) => {
                    // Escape the body for JSON embedding
                    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
                    format!("{{\"status\":{},\"body\":\"{}\"}}", status, escaped)
                }
                Err(e) => format!("{{\"error\":\"read error: {}\"}}", e),
            }
        }
        Err(e) => format!("{{\"error\":\"request failed: {}\"}}", e),
    }
}

// --- exec host function ---

/// Execute a shell command. Returns JSON result string.
/// Response: {"exit_code":0,"stdout":"...","stderr":"..."} or {"error":"..."}
pub fn wt_exec_command(cmd: impl AsRef<str>, args_json: impl AsRef<str>, cwd: impl AsRef<str>) -> String {
    // Parse args from JSON array: ["arg1", "arg2"]
    let args: Vec<String> = if args_json.as_ref().is_empty() || args_json.as_ref() == "[]" {
        Vec::new()
    } else {
        match serde_json::from_str::<Vec<String>>(args_json.as_ref()) {
            Ok(a) => a,
            Err(e) => return format!("{{\"error\":\"invalid args: {}\"}}", e),
        }
    };

    let mut command = std::process::Command::new(cmd.as_ref());
    command.args(&args);

    let cwd_str = cwd.as_ref();
    if !cwd_str.is_empty() {
        command.current_dir(cwd_str);
    }

    match command.output() {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout_escaped = stdout.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            let stderr_escaped = stderr.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            format!("{{\"exit_code\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}", exit_code, stdout_escaped, stderr_escaped)
        }
        Err(e) => format!("{{\"error\":\"exec failed: {}\"}}", e),
    }
}

// --- Daemon host functions ---

/// Get current process PID.
pub fn wt_getpid() -> i64 {
    std::process::id() as i64
}

/// Send a signal to a process. Returns 0 on success, -1 on error.
pub fn wt_kill(pid: i64, signal: i64) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let result = unsafe { libc::kill(pid as libc::pid_t, signal as libc::c_int) };
        if result == 0 { 0 } else { -1 }
    }
    #[cfg(not(unix))]
    { -1 }
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
