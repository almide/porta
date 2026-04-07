# Almide コンパイラへの依頼事項

porta (WASM agent MCP bridge) を 100% Almide で実装する過程で発見した言語側の課題。
いずれも porta の開発を直接ブロックしている。

対応する roadmap ファイルは `almide/almide` の `docs/roadmap/active/` にすでに作成済み。

---

## 1. `import self` サブモジュール解決 【最優先・ブロッカー】

**roadmap:** `docs/roadmap/active/import-self-submodule-resolution.md`

**ステータス:** 未解決（0.12.0 で再確認済み）

### 問題

`almide.toml` が存在するプロジェクトで `import self.wasm.binary` が解決されない。ファイルは発見されてパースされている（パースエラーは出ない）が、中の関数が呼び出し側から見えない。

```
porta/
  almide.toml          # [package] name = "porta"
  src/
    mod.almd           # import self.wasm.binary  ← binary.load() が見えない
    wasm/
      binary.almd      # effect fn load(...) が定義されている（単体 check は通る）
      memory.almd
      interp.almd
      wasi.almd
```

```
$ almide check src/wasm/binary.almd   # ← 単体は通る
No errors found

$ almide build src/mod.almd -o porta  # ← import 経由で呼ぶと失敗
error[E002]: undefined function 'binary.load'

$ almide compile                      # ← プロジェクトモードでも同様
error[E002]: undefined function 'binary.load'
```

`import porta.wasm.binary` でも失敗（"package 'porta' not found in dependencies"）。

### 期待動作

`docs/specs/module-system.md` の仕様通り、`src/wasm/binary.almd` は `porta.wasm.binary` として解決され、`import self.wasm.binary` → `binary.load()` で呼べるべき。

### 調査メモ

- `resolve.rs` の `find_self_module_file` はパス検索ロジックとして正しく見える
- `load_self_module` もファイル読み込み・パースを行っている
- 問題はパース後にモジュール内の関数が呼び出し側の名前空間に正しく公開されていない点にありそう

### 影響

porta は4つの `.almd` モジュールに分割して設計しているが、この問題のせいで `mod.almd` に全コードをインラインせざるを得ない。モジュール分割ができないと実質1ファイルプロジェクトになる。

### 再現

```bash
cd /Users/o6lvl4/workspace/github.com/almide/porta
almide build src/mod.almd -o porta
```

---

## 2. ~~`float.to_bits` / `int.bits_to_float` の追加~~ ✅ 解決済み

**roadmap:** `docs/roadmap/done/float-bits-conversion.md`

0.12.0 で実装済み。binary.almd で `int.bits_to_float(bits)` を使用中。

### 問題

IEEE 754 のビット表現と Float の相互変換関数がない。

### 必要な API

```almide
int.bits_to_float(bits: Int) -> Float    // i64 ビットを f64 として再解釈
float.to_bits(f: Float) -> Int           // f64 を i64 ビットとして再解釈
```

### 実装ヒント

- Rust target: `f64::from_bits(n as u64)` / `f64::to_bits() as i64`
- WASM target: `f64.reinterpret_i64` / `i64.reinterpret_f64` （1命令）

### 影響

WASM バイナリパーサが `f64.const` 命令をデコードできない。現在は `ok({val: 0.0, next: r.next})` でスタブしている。

---

## 3. ~~Variant コンストラクタを関数として渡す~~ ✅ 解決済み

**roadmap:** `docs/roadmap/done/variant-constructor-as-function.md`

0.12.0 で実装済み。

### 問題

```almide
type Instr = | Br(Int) | Call(Int) | ...

fn apply(ctor: (Int) -> Instr, v: Int) -> Instr = ctor(v)

apply(Br, 5)
// ERROR: argument 'ctor' expects fn(Int) -> Instr but got Instr
```

`Br` が `(Int) -> Instr` として扱われない。

### 期待動作

単一フィールドの Variant コンストラクタは、対応する関数型 `(T) -> Variant` として推論される。

### 影響

porta の WASM opcode ディスパッチ（~50パターン）で、同じ形の `{ let r = ...\n ok({val: Xxx(r.val), ...}) }` を繰り返し書く必要がある。コンストラクタを関数として渡せれば約30行のボイラープレートが消える。

**ワークアラウンドあり:** `(v) => Br(v)` でラムダに包めば動く。致命的ではないが冗長。

---

## 4. ~~`let ... in` 式~~ ❌ 不採用

**roadmap:** 削除済み

MSR 観点で不採用。ブロック式 `{ let x = ... \n body }` で代替。

### 問題

```almide
// これが書けない:
0x0C => let r = read(p)! in ok({val: Br(r.val), next: r.next}),

// 代わりにブロックが必要:
0x0C => {
  let r = read(p)!
  ok({val: Br(r.val), next: r.next})
},
```

### 影響

WASM opcode ディスパッチの match 式が約50 arm あり、各 arm で中間束縛が1つ必要。`let...in` がないと全 arm がブロック式になり、テーブルとしての視認性が大幅に落ちる。

**ワークアラウンドあり:** ブロック `{ ... }` で書ける。致命的ではないが ML 系言語としてはあって当然。

---

## 5. `almide compile` がプロジェクトの `almide.toml` を認識しない

**roadmap なし（#1 と関連）**

### 問題

`almide.toml` が存在するディレクトリで `almide compile` を引数なしで実行すると、プロジェクトとして認識されない。

```
$ pwd
/Users/o6lvl4/workspace/github.com/almide/porta

$ ls almide.toml
almide.toml

$ almide compile
No file specified and no almide.toml found.
Run 'almide init' to create a project, or specify a file.
```

### 期待動作

`almide.toml` がある場合、`almide compile` は `src/mod.almd` をエントリポイントとしてプロジェクト全体をコンパイルすべき。`almide run`（引数なし）も同様。

### 影響

#1 (import self) と合わせて、マルチファイルプロジェクトのビルドワークフローが完全に機能していない。ファイルを明示的に指定しても `import self` が解決されないため、プロジェクトモードが事実上使えない。

---

## 優先順

| # | 課題 | 深刻度 | 状態 |
|---|------|--------|------|
| 1 | import self サブモジュール解決 | **ブロッカー** | 未解決 |
| 5 | almide compile のプロジェクト認識 | **ブロッカー** | 未確認 |
| 2 | float bits 変換 | ~~高~~ | ✅ 解決済み |
| 3 | Variant コンストラクタ as 関数 | ~~中~~ | ✅ 解決済み |
| 4 | let...in 式 | ~~低~~ | ❌ 不採用 |

## 6. クロスモジュール Rust codegen で変数スコープが壊れる 【ブロッカー】

### 問題

`import self.wasm.binary` 経由でモジュールを統合すると、`almide check` は通るが `almide build` で Rust codegen が壊れる。サブモジュール内の関数の変数（`xs` 等）がスコープ外として参照される。

```
$ almide check src/mod.almd
No errors found

$ almide build src/mod.almd -o porta
error[E0425]: cannot find value `xs` in this scope
   --> src/main.rs:985:33
```

132 個の Rust コンパイルエラーが発生。全て `cannot find value` 系。

### 再現

```bash
cd /Users/o6lvl4/workspace/github.com/almide/porta
almide build src/mod.almd -o porta
```

mod.almd は `import self.wasm.binary` で binary.almd（706行）を使用。

---

## 優先順

| # | 課題 | 深刻度 | 状態 |
|---|------|--------|------|
| 6 | クロスモジュール Rust codegen スコープ破壊 | **ブロッカー** | 未解決 |
| 1 | import self サブモジュール解決 | ~~ブロッカー~~ | ✅ 解決済み（0.12.0） |
| 5 | almide compile のプロジェクト認識 | ~~ブロッカー~~ | ✅ 解決済み（0.12.0） |
| 2 | float bits 変換 | ~~高~~ | ✅ 解決済み |
| 3 | Variant コンストラクタ as 関数 | ~~中~~ | ✅ 解決済み |
| 4 | let...in 式 | ~~低~~ | ❌ 不採用 |

残りブロッカーは #6 のみ。check は通るので型レベルでは正常、codegen 層の問題。
