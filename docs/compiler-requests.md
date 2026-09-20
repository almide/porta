# Almide コンパイラへの依頼事項

porta (WASM agent MCP bridge) を 100% Almide で実装する過程で発見した言語側の課題。

---

## 解決済み

| # | 課題 | 状態 |
|---|------|------|
| 1 | import self サブモジュール解決 | ✅ 解決済み |
| 2 | float bits 変換 (int.bits_to_float / float.to_bits) | ✅ 解決済み |
| 3 | Variant コンストラクタを関数として渡す | ✅ 解決済み |
| 4 | let...in 式 | ❌ MSR観点で不採用 |
| 5 | almide compile のプロジェクト認識 | ✅ 解決済み |
| 6 | クロスモジュール Rust codegen スコープ破壊 | ✅ 解決済み（hex literal + variant constructor registration + Box::new） |

---

## 解決済み（追加分）

### 7. ~~Map を含む struct に PartialOrd が derive される~~ ✅ 解決済み

`Map[K, V]` をフィールドに持つ struct の Rust codegen で `#[derive(PartialOrd)]` が付くが、`HashMap` は `PartialOrd` を実装していないので Rust コンパイルが失敗する。

```almide
// src/wasm/memory.almd
type Memory = {
  pages: Map[Int, List[Int]],
  num_wasm_pages: Int,
}
```

```
error[E0277]: can't compare `HashMap<i64, Vec<i64>>` with `HashMap<i64, Vec<i64>>`
    --> src/main.rs:1806:5
     |
1804 | #[derive(Clone, Debug, PartialEq, PartialOrd)]
     |                                   ---------- in this derive macro expansion
1805 | pub struct Memory {
1806 |     pub pages: HashMap<i64, Vec<i64>>,
     |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no implementation for `HashMap<i64, Vec<i64>> < HashMap<i64, Vec<i64>>`
```

**修正案:** Map を含む型には `PartialOrd` を derive しない。`PartialEq` のみにする。あるいは Map を含むかどうかをフィールドの型から判定して derive リストを調整する。

**影響:** porta の Memory 型（ページテーブル方式の linear memory）が Map で管理されているため、interp.almd 全体がビルドできない。

### 再現

```bash
cd /Users/o6lvl4/workspace/github.com/almide/porta
almide build src/mod.almd -o porta
```

---

### 8. ~~`eprintln` が Rust マクロではなく関数呼び出しとして codegen される~~ ✅ 解決済み

`eprintln("...")` が Rust codegen で `eprintln(...)` になるが、Rust の `eprintln` はマクロなので `eprintln!(...)` が正しい。

```almide
eprintln("error occurred")
```

```
error[E0423]: expected function, found macro `eprintln`
   --> src/main.rs:xxx:9
```

**修正案:** `println` と同様に `eprintln!()` マクロ呼び出しを生成する。

**影響:** 低。`println` で代替可能。ただし stderr 出力ができない。

**ワークアラウンド:** `println` で stdout に出力する。

---

## 優先順

| # | 課題 | 状態 |
|---|------|------|
| 1 | import self サブモジュール解決 | ✅ |
| 2 | float bits 変換 | ✅ |
| 3 | Variant コンストラクタ as 関数 | ✅ |
| 4 | let...in 式 | ❌ 不採用 |
| 5 | almide compile プロジェクト認識 | ✅ |
| 6 | クロスモジュール codegen | ✅ |
| 7 | Map PartialOrd derive | ✅ |
| 8 | eprintln codegen | ✅ |

全課題解決。`porta run` が動作している。


## 0.63.0 WASM error propagation observation (2026-09-19)

During the real WASM agent/tool integration, the demo's `write_file` branch using
`fs.write(path, content)!` reported `{"ok":"wrote 19 bytes to result.txt"}` even
though a read-only WASI preopen correctly prevented creation of the file.
Replacing that call with an explicit `match fs.write(...) { ok(_) => ..., err(e)
=> ... }` produced the expected error result. `read_file` now uses the same
explicit error handling. This is an observed behavior difference under the
published v0.63.0-rc1 compiler; its precise compiler cause is not established.

The regression is exercised by `scripts/agent_integration.py`: it checks both
actual filesystem state and the tool error delivered back to the model. Do not
weaken the test to accept a false-success tool response. No upstream source was
modified as part of this workaround.

## 0.63.0 stock-WASI JSON artifact checker wall (2026-09-19)

`scripts/fixtures/verify_json_compiler_repro.almd` reads a JSON-line request,
reads a file through `fs.read_text`, parses that file as JSON, and prints a
verification verdict. With the pinned v0.63.0-rc1 compiler, stock-WASI build
refuses this shape: the incumbent renderer reports `heap-result match outside
the executable subset`, and the structural renderer reports `host op 1 has no
stock-WASI service`. Simplifying result-returning helpers and inlining the match
branches did not eliminate the refusal. The exact compiler cause is not established.

Reproduce with `almide build scripts/fixtures/verify_json_compiler_repro.almd --target wasm -o /tmp/verify-repro.wasm`. No unverified renderer fallback is used.
The working sample `examples/verify-json` instead compiles Rust/serde_json to
`wasm32-wasip1`; the Porta CLI, agent loop and permission probes stay on Almide
0.63.0. The completion protocol accepts standard WASI artifacts independently
of the guest implementation language.
