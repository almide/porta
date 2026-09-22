//! Reading a module's imports and exports without running it.
//!
//! `porta inspect` and the capability check both need to know what a module
//! asks of its host, and neither may instantiate it to find out. A WASI 0.2
//! component is read the same way: its imports are interfaces
//! (`wasi:cli/stdout@0.2.3`) rather than functions of one module, and they
//! are reported under `module` with an empty `name`, so the capability check
//! sees one list whichever it was handed.

use wasmtime::component::{types::ComponentItem, Component};
use wasmtime::{Config, Engine, ExternType, Module};

/// Inspect a WASM module or component: extract imports and exports as JSON.
/// Returns JSON string with {component, imports: [...], exports: [...], memories: [...]}
pub fn wt_inspect(wasm_path: impl AsRef<str>) -> String {
    let bytes = match std::fs::read(wasm_path.as_ref()) {
        Ok(b) => b,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };
    let mut config = Config::new();
    config.wasm_component_model(true);
    // The same features the runner enables, or a WASI 0.3 component would be
    // unreadable here and its imports invisible to the capability check.
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    let engine = match Engine::new(&config) {
        Ok(engine) => engine,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };
    if crate::wasmtime_bridge::is_component(&bytes) {
        return inspect_component(&engine, &bytes);
    }
    inspect_module(&engine, &bytes)
}

fn extern_kind(ty: &ExternType) -> &'static str {
    match ty {
        ExternType::Func(_) => "func",
        ExternType::Table(_) => "table",
        ExternType::Memory(_) => "memory",
        ExternType::Global(_) => "global",
        _ => "unknown",
    }
}

fn inspect_module(engine: &Engine, bytes: &[u8]) -> String {
    let module = match Module::from_binary(engine, bytes) {
        Ok(m) => m,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };
    let imports: Vec<String> = module
        .imports()
        .map(|imp| format!("{{\"module\":\"{}\",\"name\":\"{}\",\"kind\":\"{}\"}}", imp.module(), imp.name(), extern_kind(&imp.ty())))
        .collect();
    let exports: Vec<String> = module
        .exports()
        .map(|exp| format!("{{\"name\":\"{}\",\"kind\":\"{}\"}}", exp.name(), extern_kind(&exp.ty())))
        .collect();
    let memories: Vec<String> = module
        .exports()
        .filter_map(|exp| match exp.ty() {
            ExternType::Memory(m) => Some(format!("{{\"min\":{}}}", m.minimum())),
            _ => None,
        })
        .collect();
    format!(
        "{{\"component\":false,\"imports\":[{}],\"exports\":[{}],\"memories\":[{}]}}",
        imports.join(","),
        exports.join(","),
        memories.join(","),
    )
}

fn item_kind(item: &ComponentItem) -> &'static str {
    match item {
        ComponentItem::ComponentFunc(_) => "func",
        ComponentItem::CoreFunc(_) => "core func",
        ComponentItem::Module(_) => "module",
        ComponentItem::Component(_) => "component",
        ComponentItem::ComponentInstance(_) => "instance",
        ComponentItem::Type(_) => "type",
        _ => "resource",
    }
}

fn inspect_component(engine: &Engine, bytes: &[u8]) -> String {
    let component = match Component::from_binary(engine, bytes) {
        Ok(c) => c,
        Err(e) => return format!("{{\"error\":\"{}\"}}", e),
    };
    let ty = component.component_type();
    let imports: Vec<String> = ty
        .imports(engine)
        .map(|(name, item)| format!("{{\"module\":\"{}\",\"name\":\"\",\"kind\":\"{}\"}}", crate::json_text::escape_json_text(name), item_kind(&item.ty)))
        .collect();
    let exports: Vec<String> = ty
        .exports(engine)
        .map(|(name, item)| format!("{{\"name\":\"{}\",\"kind\":\"{}\"}}", crate::json_text::escape_json_text(name), item_kind(&item.ty)))
        .collect();
    // The WASI version is the one its imports name; a component importing no
    // wasi: interface at all is reported as 0.2, the sync world it would use.
    let wasi = if ty.imports(engine).any(|(name, _)| name.starts_with("wasi:") && name.contains("@0.3")) { "0.3" } else { "0.2" };
    format!(
        "{{\"component\":true,\"wasi\":\"{}\",\"imports\":[{}],\"exports\":[{}],\"memories\":[]}}",
        wasi,
        imports.join(","),
        exports.join(",")
    )
}
