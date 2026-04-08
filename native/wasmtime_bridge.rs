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

    // Try loading precompiled cache
    let cache_path = format!("{}.porta-cache", path);
    let module = if let Ok(cached) = std::fs::read(&cache_path) {
        unsafe { Module::deserialize(&engine, &cached) }.ok()
    } else {
        None
    };
    let module = match module {
        Some(m) => m,
        None => {
            // Compile and cache
            match Module::from_binary(&engine, &bytes) {
                Ok(m) => {
                    if let Ok(serialized) = m.serialize() {
                        let _ = std::fs::write(&cache_path, &serialized);
                    }
                    m
                }
                Err(_) => return -1,
            }
        }
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

/// Set WASI command-line arguments (must be called before wt_run).
/// args_json: JSON array of strings, e.g. ["python.wasm", "script.py"]
pub fn wt_set_args(handle: i64, args_json: impl AsRef<str>) -> i64 {
    let args: Vec<String> = match serde_json::from_str(args_json.as_ref()) {
        Ok(a) => a,
        Err(_) => return -1,
    };
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.wasi_args = args; 0 }
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

/// Set maximum memory in WASM pages (64KB each). 0 = unlimited.
pub fn wt_set_max_memory(handle: i64, pages: i64) -> i64 {
    let mut instances = INSTANCES.lock().unwrap();
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
    let mut instances = INSTANCES.lock().unwrap();
    match instances.get_mut(handle as usize).and_then(|s| s.as_mut()) {
        Some(inst) => { inst.entry_point = name.as_ref().to_string(); 0 }
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
    if !inst.wasi_args.is_empty() {
        wasi.args(&inst.wasi_args);
    }
    for (k, v) in &inst.env_vars {
        wasi.env(k, v);
    }
    // Always set stdin (empty = immediate EOF for non-interactive mode)
    wasi.stdin(MemoryInputPipe::new(inst.stdin_data.clone()));
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
    let limits = if inst.max_memory_bytes > 0 {
        StoreLimitsBuilder::new().memory_size(inst.max_memory_bytes).build()
    } else {
        StoreLimitsBuilder::new().build()
    };
    let ctx = PortaCtx { wasi: wasi_ctx, limits };
    let mut store = Store::new(&inst.engine, ctx);
    store.limiter(|ctx| &mut ctx.limits);

    if inst.fuel > 0 {
        let _ = store.set_fuel(inst.fuel);
    }

    let mut linker = Linker::new(&inst.engine);
    if let Err(e) = p1::add_to_linker_sync(&mut linker, |ctx: &mut PortaCtx| &mut ctx.wasi) {
        inst.stderr_result = format!("linker setup failed: {}", e);
        inst.exit_code = -1;
        return -1;
    }

    // Register porta host functions for WASM agents
    // porta.http_request(req_ptr, req_len, resp_ptr, resp_cap) -> resp_len
    // Agent writes JSON request to memory, gets JSON response back
    let _ = linker.func_wrap(
        "porta",
        "http_request",
        |mut caller: Caller<'_, PortaCtx>, req_ptr: i32, req_len: i32, resp_ptr: i32, resp_cap: i32| -> i32 {
            let memory = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return -1,
            };
            let data = memory.data(&caller);
            let req_bytes = &data[req_ptr as usize..(req_ptr + req_len) as usize];
            let req_str = std::str::from_utf8(req_bytes).unwrap_or("");

            // Parse JSON: {"method":"GET","url":"...","headers":{...},"body":"..."}
            let resp_str = if let Ok(req) = serde_json::from_str::<serde_json::Value>(req_str) {
                let method = req["method"].as_str().unwrap_or("GET");
                let url = req["url"].as_str().unwrap_or("");
                let headers_val = &req["headers"];
                let body = req["body"].as_str().unwrap_or("");

                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build();
                match client {
                    Ok(client) => {
                        let mut builder = match method {
                            "POST" => client.post(url),
                            "PUT" => client.put(url),
                            "DELETE" => client.delete(url),
                            "PATCH" => client.patch(url),
                            _ => client.get(url),
                        };
                        if let Some(obj) = headers_val.as_object() {
                            for (k, v) in obj {
                                if let Some(s) = v.as_str() {
                                    builder = builder.header(k.as_str(), s);
                                }
                            }
                        }
                        if !body.is_empty() { builder = builder.body(body.to_string()); }
                        match builder.send() {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                let text = resp.text().unwrap_or_default();
                                let escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
                                format!("{{\"status\":{},\"body\":\"{}\"}}", status, escaped)
                            }
                            Err(e) => format!("{{\"error\":\"{}\"}}", e),
                        }
                    }
                    Err(e) => format!("{{\"error\":\"{}\"}}", e),
                }
            } else {
                "{\"error\":\"invalid request JSON\"}".to_string()
            };

            let resp_bytes = resp_str.as_bytes();
            let write_len = resp_bytes.len().min(resp_cap as usize);
            let mem_data = memory.data_mut(&mut caller);
            mem_data[resp_ptr as usize..resp_ptr as usize + write_len].copy_from_slice(&resp_bytes[..write_len]);
            write_len as i32
        },
    );

    // porta.exec_command(req_ptr, req_len, resp_ptr, resp_cap) -> resp_len
    let _ = linker.func_wrap(
        "porta",
        "exec_command",
        |mut caller: Caller<'_, PortaCtx>, req_ptr: i32, req_len: i32, resp_ptr: i32, resp_cap: i32| -> i32 {
            let memory = match caller.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => return -1,
            };
            let data = memory.data(&caller);
            let req_bytes = &data[req_ptr as usize..(req_ptr + req_len) as usize];
            let req_str = std::str::from_utf8(req_bytes).unwrap_or("");

            let resp_str = if let Ok(req) = serde_json::from_str::<serde_json::Value>(req_str) {
                let cmd = req["command"].as_str().unwrap_or("");
                let args: Vec<String> = req["args"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                let cwd = req["cwd"].as_str().unwrap_or(".");

                match std::process::Command::new(cmd).args(&args).current_dir(cwd).output() {
                    Ok(output) => {
                        let exit_code = output.status.code().unwrap_or(-1);
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let so = stdout.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
                        let se = stderr.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
                        format!("{{\"exit_code\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}", exit_code, so, se)
                    }
                    Err(e) => format!("{{\"error\":\"{}\"}}", e),
                }
            } else {
                "{\"error\":\"invalid request JSON\"}".to_string()
            };

            let resp_bytes = resp_str.as_bytes();
            let write_len = resp_bytes.len().min(resp_cap as usize);
            let mem_data = memory.data_mut(&mut caller);
            mem_data[resp_ptr as usize..resp_ptr as usize + write_len].copy_from_slice(&resp_bytes[..write_len]);
            write_len as i32
        },
    );

    let instance = match linker.instantiate(&mut store, &inst.module) {
        Ok(i) => i,
        Err(e) => {
            inst.stderr_result = format!("instantiation failed: {}", e);
            inst.exit_code = -1;
            return -1;
        }
    };

    // Try () -> () first, then () -> i32 (for effect fn main returning Result)
    let entry = &inst.entry_point;
    let result = if let Ok(start) = instance.get_typed_func::<(), ()>(&mut store, entry) {
        start.call(&mut store, ())
    } else if let Ok(start) = instance.get_typed_func::<(), (i32,)>(&mut store, entry) {
        start.call(&mut store, ()).map(|_| ())
    } else {
        inst.stderr_result = format!("{} function not found", entry);
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

// --- Functions below migrated to pure Almide (kept only for linker host functions) ---

// NOTE: wt_http_request, wt_exec_command, wt_exec_sandboxed, wt_getpid,
// wt_kill, wt_spawn, wt_home_dir are now implemented in src/wasm_rt.almd
// The Rust versions below are ONLY used by wasmtime linker host functions.

// --- HTTP (used by linker host function only) ---

/// Execute an HTTP request. Returns JSON response string.
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

/// Execute a command inside an OS-level sandbox.
/// Returns JSON: {"exit_code":0,"stdout":"...","stderr":"..."} or {"error":"..."}
pub fn wt_exec_sandboxed(
    cmd: impl AsRef<str>,
    args_json: impl AsRef<str>,
    allowed_dirs_json: impl AsRef<str>,
    allowed_net_json: impl AsRef<str>,
    env_json: impl AsRef<str>,
    cwd: impl AsRef<str>,
) -> String {
    let args: Vec<String> = serde_json::from_str(args_json.as_ref()).unwrap_or_default();
    let allowed_dirs_raw: Vec<String> = serde_json::from_str(allowed_dirs_json.as_ref()).unwrap_or_default();
    // Resolve paths, strip :ro suffix for resolution but keep it for sandbox profile
    let allowed_dirs: Vec<String> = allowed_dirs_raw.iter().map(|d| {
        let clean = d.trim_end_matches(":ro");
        let abs = if clean.starts_with('/') {
            clean.to_string()
        } else {
            std::fs::canonicalize(clean).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| clean.to_string())
        };
        if d.ends_with(":ro") { format!("{}:ro", abs) } else { abs }
    }).collect();
    let allowed_net: Vec<String> = serde_json::from_str(allowed_net_json.as_ref()).unwrap_or_default();
    let env_vars: Vec<(String, String)> = serde_json::from_str::<Vec<Vec<String>>>(env_json.as_ref())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pair| {
            if pair.len() == 2 { Some((pair[0].clone(), pair[1].clone())) } else { None }
        })
        .collect();

    #[cfg(target_os = "macos")]
    {
        exec_sandboxed_macos(cmd.as_ref(), &args, &allowed_dirs, &allowed_net, &env_vars, cwd.as_ref())
    }
    #[cfg(target_os = "linux")]
    {
        exec_sandboxed_linux(cmd.as_ref(), &args, &allowed_dirs, &allowed_net, &env_vars, cwd.as_ref())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "{\"error\":\"sandboxed execution not supported on this platform\"}".to_string()
    }
}

#[cfg(target_os = "macos")]
fn exec_sandboxed_macos(
    cmd: &str, args: &[String], allowed_dirs: &[String], allowed_net: &[String],
    env_vars: &[(String, String)], cwd: &str,
) -> String {
    // Build sandbox-exec profile
    // Strategy: allow default + deny writes outside allowed dirs + deny reads on sensitive dirs
    let mut profile = String::from("(version 1)\n(allow default)\n");

    // --- FS write: deny all, allow only specified dirs + system essentials ---
    profile.push_str("(deny file-write*)\n");
    for dir in allowed_dirs.iter() {
        let clean = dir.trim_end_matches(":ro");
        if !dir.ends_with(":ro") {
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", clean));
        }
    }
    // System paths needed by runtimes
    profile.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/var\"))\n");
    profile.push_str("(allow file-write* (subpath \"/var\"))\n");
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    if let Ok(home) = std::env::var("HOME") {
        profile.push_str(&format!("(allow file-write* (subpath \"{}/Library\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.config\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.cache\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.npm\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.claude\"))\n", home));
    }

    // --- FS read: deny sensitive directories ---
    if let Ok(home) = std::env::var("HOME") {
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.ssh\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.gnupg\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.aws\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.kube\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.docker\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Documents\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Desktop\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Downloads\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Pictures\"))\n", home));
    }

    // --- Network restrictions ---
    if !allowed_net.is_empty() {
        profile.push_str("(deny network-outbound)\n");
        profile.push_str("(allow network-outbound (local udp))\n");
        profile.push_str("(allow network-outbound (remote unix-socket))\n");
        for host in allowed_net {
            if let Some(colon) = host.rfind(':') {
                let port = &host[colon + 1..];
                profile.push_str(&format!("(allow network-outbound (remote tcp \"*:{}\"))\n", port));
            } else {
                profile.push_str("(allow network-outbound (remote tcp \"*:*\"))\n");
            }
        }
    }

    let mut command = std::process::Command::new("sandbox-exec");
    command.arg("-p").arg(&profile).arg(cmd).args(args);
    if !cwd.is_empty() {
        command.current_dir(cwd);
    }
    for (k, v) in env_vars {
        command.env(k, v);
    }

    match command.output() {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let so = stdout.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            let se = stderr.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            format!("{{\"exit_code\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}", exit_code, so, se)
        }
        Err(e) => format!("{{\"error\":\"sandbox exec failed: {}\"}}", e),
    }
}

#[cfg(target_os = "linux")]
fn exec_sandboxed_linux(
    cmd: &str, args: &[String], allowed_dirs: &[String], _allowed_net: &[String],
    env_vars: &[(String, String)], cwd: &str,
) -> String {
    // Linux: use unshare if available, fallback to direct exec with chroot-like restriction
    // For now, basic implementation without root (no namespace)
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    if !cwd.is_empty() {
        command.current_dir(cwd);
    }
    for (k, v) in env_vars {
        command.env(k, v);
    }
    // TODO: Add unshare/seccomp when running as root

    match command.output() {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let so = stdout.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            let se = stderr.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
            format!("{{\"exit_code\":{},\"stdout\":\"{}\",\"stderr\":\"{}\"}}", exit_code, so, se)
        }
        Err(e) => format!("{{\"error\":\"exec failed: {}\"}}", e),
    }
}

/// Replace the current process with a sandboxed command (Unix exec).
/// This function never returns on success — porta becomes the sandboxed process.
/// On failure, returns a JSON error string.
pub fn wt_exec_replace(
    cmd: impl AsRef<str>,
    args_json: impl AsRef<str>,
    allowed_dirs_json: impl AsRef<str>,
    allowed_net_json: impl AsRef<str>,
    env_json: impl AsRef<str>,
    cwd: impl AsRef<str>,
) -> String {
    let args: Vec<String> = serde_json::from_str(args_json.as_ref()).unwrap_or_default();
    let allowed_dirs_raw: Vec<String> = serde_json::from_str(allowed_dirs_json.as_ref()).unwrap_or_default();
    let allowed_dirs: Vec<String> = allowed_dirs_raw.iter().map(|d| {
        let clean = d.trim_end_matches(":ro");
        let abs = if clean.starts_with('/') {
            clean.to_string()
        } else {
            std::fs::canonicalize(clean).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| clean.to_string())
        };
        if d.ends_with(":ro") { format!("{}:ro", abs) } else { abs }
    }).collect();
    let allowed_net: Vec<String> = serde_json::from_str(allowed_net_json.as_ref()).unwrap_or_default();
    let env_vars: Vec<(String, String)> = serde_json::from_str::<Vec<Vec<String>>>(env_json.as_ref())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|pair| {
            if pair.len() == 2 { Some((pair[0].clone(), pair[1].clone())) } else { None }
        })
        .collect();

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        let profile = build_sandbox_profile_rs(&allowed_dirs, &allowed_net);
        let mut command = std::process::Command::new("sandbox-exec");
        command.arg("-p").arg(&profile).arg(cmd.as_ref()).args(&args);
        if !cwd.as_ref().is_empty() && cwd.as_ref() != "." {
            command.current_dir(cwd.as_ref());
        }
        for (k, v) in &env_vars {
            command.env(k, v);
        }
        // exec() replaces the current process — never returns on success
        let err = command.exec();
        format!("{{\"error\":\"exec failed: {}\"}}", err)
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!("{{\"error\":\"exec_replace not supported on this platform\"}}")
    }
}

/// Shared sandbox profile builder for Rust-side exec functions.
#[cfg(target_os = "macos")]
fn build_sandbox_profile_rs(allowed_dirs: &[String], allowed_net: &[String]) -> String {
    let mut profile = String::from("(version 1)\n(allow default)\n");
    profile.push_str("(deny file-write*)\n");
    for dir in allowed_dirs.iter() {
        let clean = dir.trim_end_matches(":ro");
        if !dir.ends_with(":ro") {
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", clean));
        }
    }
    profile.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/var\"))\n");
    profile.push_str("(allow file-write* (subpath \"/var\"))\n");
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    if let Ok(home) = std::env::var("HOME") {
        profile.push_str(&format!("(allow file-write* (subpath \"{}/Library\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.config\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.cache\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.npm\"))\n", home));
        profile.push_str(&format!("(allow file-write* (subpath \"{}/.claude\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.ssh\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.gnupg\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.aws\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.kube\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/.docker\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Documents\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Desktop\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Downloads\"))\n", home));
        profile.push_str(&format!("(deny file-read-data (subpath \"{}/Pictures\"))\n", home));
    }
    profile.push_str("(deny network-outbound)\n");
    profile.push_str("(allow network-outbound (local udp))\n");
    profile.push_str("(allow network-outbound (remote unix-socket))\n");
    if !allowed_net.is_empty() {
        for host in allowed_net {
            if let Some(colon) = host.rfind(':') {
                let port = &host[colon + 1..];
                profile.push_str(&format!("(allow network-outbound (remote tcp \"*:{}\"))\n", port));
            } else {
                profile.push_str("(allow network-outbound (remote tcp \"*:*\"))\n");
            }
        }
    }
    profile
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
    let mut instances = INSTANCES.lock().unwrap();
    let idx = handle as usize;
    if idx < instances.len() {
        instances[idx] = None;
        0
    } else {
        -1
    }
}
