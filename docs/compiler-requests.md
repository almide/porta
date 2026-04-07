# Almide コンパイラへの依頼事項

porta (WASM agent MCP bridge) を 100% Almide で実装する過程で発見した言語側の課題。
いずれも porta の開発を直接ブロックしている。

対応する roadmap ファイルは `almide/almide` の `docs/roadmap/active/` にすでに作成済み。

---

## 1. `import self` サブモジュール解決 【最優先・ブロッカー】

**roadmap:** `docs/roadmap/active/import-self-submodule-resolution.md`

### 問題

`almide.toml` が存在するプロジェクトで `import self.wasm.binary` が解決されない。

```
porta/
  almide.toml          # [package] name = "porta"
  src/
    mod.almd           # import self.wasm.binary  ← ここで失敗
    wasm/
      binary.almd      # effect fn load(...) が定義されている
      memory.almd
      interp.almd
      wasi.almd
```

```
$ almide build src/mod.almd -o porta
error[E002]: undefined function 'binary.load'
```

### 期待動作

`docs/specs/module-system.md` の仕様通り、`src/wasm/binary.almd` は `porta.wasm.binary` として解決され、`import self.wasm.binary` → `binary.load()` で呼べるべき。

### 影響

porta は4つの `.almd` モジュールに分割して設計しているが、この問題のせいで `mod.almd` に全コードをインラインせざるを得ない。モジュール分割ができないと実質1ファイルプロジェクトになる。

### 再現

```bash
cd /Users/o6lvl4/workspace/github.com/almide/porta
almide build src/mod.almd -o porta
```

---

## 2. `float.to_bits` / `int.bits_to_float` の追加

**roadmap:** `docs/roadmap/active/float-bits-conversion.md`

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

## 3. Variant コンストラクタを関数として渡す

**roadmap:** `docs/roadmap/active/variant-constructor-as-function.md`

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

## 4. `let ... in` 式

**roadmap:** `docs/roadmap/active/let-in-expression.md`

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

## 優先順

| # | 課題 | 深刻度 | ワークアラウンド |
|---|------|--------|-----------------|
| 1 | import self サブモジュール解決 | **ブロッカー** | 1ファイルにインライン（限界あり） |
| 2 | float bits 変換 | 高 | `0.0` スタブ（f64.const が壊れる） |
| 3 | Variant コンストラクタ as 関数 | 中 | ラムダで包む |
| 4 | let...in 式 | 低 | ブロック式で代替 |

1 が直れば porta は即座にモジュール分割できる。2 が直れば WASM パーサが完全になる。3, 4 は QoL 改善。
