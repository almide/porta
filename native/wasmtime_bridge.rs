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
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};

struct WasmInstance {
    engine: Engine,
    module: Module,
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
pub use crate::sandbox_exec::{
    wt_exec_replace, wt_exec_sandboxed, wt_exec_supervised, wt_parse_toml, wt_sandbox_profile,
};

/// Create a WASM instance from a file path.
/// Returns handle (>= 0) on success, -1 on error.
pub fn wt_create(wasm_path: impl AsRef<str>, fuel: i64) -> i64 {
    let path = wasm_path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return -1,
    };

    let mut config = Config::new();
    if fuel > 0 {
        config.consume_fuel(true);
    }
    config.wasm_multi_memory(true);

    let engine = match Engine::new(&config) {
        Ok(e) => e,
        Err(_) => return -1,
    };

    // Serialized native modules are executable code, not untrusted WASM.
    // Never deserialize an attacker-writable sidecar next to an agent.
    let module = match Module::from_binary(&engine, &bytes) {
        Ok(module) => module,
        Err(_) => return -1,
    };

    let inst = WasmInstance {
        engine,
        module,
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

// --- Native services invoked by checked Almide MCP handlers ---

/// Execute an HTTP request. Returns JSON response string.
pub fn wt_http_request(method: impl AsRef<str>, url: impl AsRef<str>, headers_json: impl AsRef<str>, body: impl AsRef<str>) -> String {
    let client = match checked_client() {
        Ok(client) => client,
        Err(reason) => return format!("{{\"error\":\"client error: {}\"}}", reason),
    };
    let Some(mut request) = request_for(&client, method.as_ref(), url.as_ref()) else {
        return format!("{{\"error\":\"unsupported method: {}\"}}", method.as_ref());
    };
    request = with_headers(request, headers_json.as_ref());
    if !body.as_ref().is_empty() { request = request.body(body.as_ref().to_string()); }
    match request.send() {
        Ok(response) => encoded_response(response),
        Err(error) => format!("{{\"error\":\"request failed: {}\"}}", error),
    }
}

/// A client that follows no redirect and reads no proxy variable: where a
/// request may go is decided by the caller's policy, never by the environment.
fn checked_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

/// The builder for a method this host function supports, or nothing.
fn request_for(client: &reqwest::blocking::Client, method: &str, url: &str) -> Option<reqwest::blocking::RequestBuilder> {
    match method {
        "GET" => Some(client.get(url)),
        "POST" => Some(client.post(url)),
        "PUT" => Some(client.put(url)),
        "DELETE" => Some(client.delete(url)),
        "PATCH" => Some(client.patch(url)),
        "HEAD" => Some(client.head(url)),
        _ => None,
    }
}

/// Adds the caller's headers, given as a JSON object of string values. Anything
/// else carries no header, which is how this ABI has always answered.
fn with_headers(mut request: reqwest::blocking::RequestBuilder, headers_json: &str) -> reqwest::blocking::RequestBuilder {
    let Ok(headers) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(headers_json) else {
        return request;
    };
    for (name, value) in headers {
        if let Some(text) = value.as_str() { request = request.header(name.as_str(), text); }
    }
    request
}

fn encoded_response(response: reqwest::blocking::Response) -> String {
    let status = response.status().as_u16();
    match response.text() {
        Ok(text) => format!("{{\"status\":{},\"body\":\"{}\"}}", status, escape_json_text(&text)),
        Err(error) => format!("{{\"error\":\"read error: {}\"}}", error),
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

/// Inspect a WASM module: extract imports and exports as JSON.
/// Returns JSON string with {imports: [...], exports: [...]}
pub fn wt_inspect(wasm_path: impl AsRef<str>) -> String {
    let bytes = match std::fs::read(wasm_path.as_ref()) {
        Ok(b) => b,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };

    let engine = Engine::default();

    let module = match Module::from_binary(&engine, &bytes) {
        Ok(m) => m,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };

    let imports: Vec<String> = module.imports().map(|imp| {
        let kind = match imp.ty() {
            ExternType::Func(_) => "func",
            ExternType::Table(_) => "table",
            ExternType::Memory(_) => "memory",
            ExternType::Global(_) => "global",
            _ => "unknown",
        };
        format!("{{\"module\":\"{}\",\"name\":\"{}\",\"kind\":\"{}\"}}", imp.module(), imp.name(), kind)
    }).collect();

    let exports: Vec<String> = module.exports().map(|exp| {
        let kind = match exp.ty() {
            ExternType::Func(_) => "func",
            ExternType::Table(_) => "table",
            ExternType::Memory(_) => "memory",
            ExternType::Global(_) => "global",
            _ => "unknown",
        };
        format!("{{\"name\":\"{}\",\"kind\":\"{}\"}}", exp.name(), kind)
    }).collect();

    let types_count = module.imports().count() + module.exports().count();
    let memories: Vec<String> = module.exports().filter_map(|exp| {
        match exp.ty() {
            ExternType::Memory(m) => Some(format!("{{\"min\":{}}}", m.minimum())),
            _ => None,
        }
    }).collect();

    format!(
        "{{\"imports\":[{}],\"exports\":[{}],\"memories\":[{}]}}",
        imports.join(","),
        exports.join(","),
        memories.join(","),
    )
}


/// Spawn a detached process. Returns PID (>0) or -1 on error.
pub fn wt_spawn(cmd: impl AsRef<str>, args_json: impl AsRef<str>) -> i64 {
    let args: Vec<String> = if args_json.as_ref().is_empty() || args_json.as_ref() == "[]" {
        Vec::new()
    } else {
        match serde_json::from_str::<Vec<String>>(args_json.as_ref()) {
            Ok(a) => a,
            Err(_) => return -1,
        }
    };

    match std::process::Command::new(cmd.as_ref())
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child.id() as i64,
        Err(_) => -1,
    }
}

/// Get HOME directory path.
pub fn wt_home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
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


