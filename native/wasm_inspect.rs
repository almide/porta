//! Reading a module's imports and exports without running it.
//!
//! `porta inspect` and the capability check both need to know what a module
//! asks of its host, and neither may instantiate it to find out.

use wasmtime::{Config, Engine, ExternType, Module};

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
