<!-- description: Always validate WASM imports, never skip on empty module -->
<!-- done: 2026-04-08 -->

# Always Validate Imports

**Priority: Critical**

## Problem

`dispatch.dispatch_tool` and `run_with_restarts` skip `validate_imports` when `config.wasm.imports` is empty. But the common paths (`serve` with manifest, `run`) use `empty_wasm()` which has empty imports.

Result: **the normal usage path has no import validation**.

## Fix

Use `wt_inspect` (native, handles any size) to get real imports, then validate.

### In dispatch.almd
```
// Before: skip if empty
if list.len(config.wasm.imports) > 0 then validate...

// After: always inspect and validate
let info = wasm_rt.wt_inspect(config.wasm_path)
let imports = parse_imports(info)
sandbox.validate_imports(imports, config.capabilities)!
```

### Remove empty_wasm() pattern
Stop using `empty_wasm()` as a shortcut. Always have real import data.

## Files
- `src/dispatch.almd` — always validate using wt_inspect
- `src/mod.almd` — remove empty_wasm(), build_run_config
