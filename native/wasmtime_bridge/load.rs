//! Reading one `.wasm` file into something the runner can instantiate: the
//! engine configuration, the module-or-component decision, and the store
//! data a component run needs.

use super::*;

/// What a `.wasm` file turned out to be. A core module runs through WASI
/// preview 1 and its `_start`; a component runs through the component linker
/// and its `wasi:cli/run`. All get the same capability check, the same
/// preopens, the same fuel and memory limits.
pub(super) enum Code {
    Module(Module),
    /// `p3` marks a WASI 0.3 component: its imports are the 0.3 interfaces
    /// and its `run` is async-lifted, so it goes through the async linker.
    Component { component: component::Component, p3: bool },
}

/// The store data for a component run: the WASI context and the resource
/// table its handles live in, under the same memory limiter as a module.
pub(super) struct ComponentCtx {
    pub(super) wasi: WasiCtx,
    pub(super) table: ResourceTable,
    pub(super) limits: StoreLimits,
}

impl WasiView for ComponentCtx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

/// A component is a core module with one more layer; the header's layer field
/// tells them apart before either parser is asked. After the magic come a
/// 16-bit version (0x000d for components) and a 16-bit layer (1 = component).
pub(crate) fn is_component(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..4] == *b"\0asm" && bytes[4..8] == [0x0d, 0x00, 0x01, 0x00]
}

/// Whether a component speaks WASI 0.3: any `wasi:` import at that version.
fn imports_wasi_0_3(engine: &Engine, component: &component::Component) -> bool {
    component.component_type().imports(engine).any(|(name, _)| name.starts_with("wasi:") && name.contains("@0.3"))
}

/// The engine every instance runs on. Fuel metering only when a budget was
/// asked for; the component model with its async ABI and stream and future
/// builtins always, since a 0.2 component or a core module ignores them and
/// a 0.3 component cannot load without them.
pub(super) fn engine_config(fuel: i64) -> Config {
    let mut config = Config::new();
    if fuel > 0 {
        config.consume_fuel(true);
    }
    config.wasm_multi_memory(true);
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_more_async_builtins(true);
    config.concurrency_support(true);
    config
}

/// Parses the bytes as whichever the header says they are. Serialized native
/// modules are executable code, not untrusted WASM: nothing here ever
/// deserializes an attacker-writable sidecar next to an agent.
pub(super) fn load_code(engine: &Engine, bytes: &[u8]) -> Option<Code> {
    if is_component(bytes) {
        let component = component::Component::from_binary(engine, bytes).ok()?;
        let p3 = imports_wasi_0_3(engine, &component);
        return Some(Code::Component { component, p3 });
    }
    Module::from_binary(engine, bytes).ok().map(Code::Module)
}
