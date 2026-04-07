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

## 未解決

### 7. Map を含む struct に PartialOrd が derive される 【ブロッカー】

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

### 8. `eprintln` が Rust マクロではなく関数呼び出しとして codegen される

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

| # | 課題 | 深刻度 | 状態 |
|---|------|--------|------|
| 7 | Map を含む struct の PartialOrd derive | **ブロッカー** | 未解決 |
| 8 | eprintln codegen | 低 | 未解決 |

#7 が直れば `porta run` が動く。
