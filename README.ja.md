<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  実際に与えた権限だけでエージェントを動かす。<br>
  今使っている CLI エージェントには OS レベルの制限を、これから作るエージェントには WASM ランタイムを。
</p>

<p align="center">
  <a href="https://github.com/almide/almide">Almide</a> + <a href="https://wasmtime.dev">Wasmtime</a> · Docker 不要
</p>

<p align="center">
  <a href="README.md">English</a> · 日本語
</p>

---

## こんな人向け

- **CLI エージェントを使っていて**、指定したディレクトリの外に書かせたくない、
  許可していないホストに繋がせたくない。しかもプロンプトでのお願いではなく
  OS に止めさせたい。
- **チームにエージェントを配っていて**、どこに接続しようとしたかの記録が要る。
- **エージェントを作っていて**、クラッシュ後に作業を重複させずに再開したい。
  エージェントが言い逃れできない完了チェックが欲しい。

どれも自分の問題ではない場合、Porta は面白くありません。これはランタイムと
制限の仕組みであって、エージェントそのものではありません。

## インストール

```bash
bash scripts/install-almide.sh
.tools/almide/almide build src/mod.almd -o target/porta
cp target/porta ~/.local/bin/
```

Almide 0.63.0、Rust ツールチェーン、Python 3、curl が必要です。検証手順と
コンパイラのピン留めについては[ソースからのビルド](#ソースからのビルド)を参照してください。

## クイックスタート

### 1. 今使っているエージェントを制限する

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -e "HOME=$HOME" \
  -- --print "Fix the bug in main.rs"
```

`claude` は通常どおり動きますが、`./project` の中にしか書き込めず、指定した
ホストにしか接続できません。Docker デーモンもコンテナイメージも不要で、
エージェント側の変更も要りません。

### 2. 制限が実際に止める様子を見る

効いている制限は何も起きないので、止まる瞬間を見てください。

```bash
porta run curl -- https://example.com                     # ネットワークは既定で開放
porta run curl --allow-net '*:443' -- https://example.com # HTTPS 許可 → 成功
porta run curl --allow-net '*:80'  -- https://example.com # → 終了コード 7、443 は拒否
```

どこへ接続しようとしたかを記録する場合:

```bash
porta run claude --proxy-allow 'api.anthropic.com' --proxy-audit egress.jsonl \
  -v ./project -- --print "..."
```

### 3. 設定を毎回打たずに保存する

```bash
porta init native claude   # porta.toml を生成
porta up -- --print "Fix the bug in main.rs"
```

### 4. WASM の中で動くエージェントを作る

```bash
.tools/almide/almide build examples/chat-agent/src/mod.almd --target wasm -o examples/chat-agent/agent.wasm
# examples/chat-agent/agent.toml にモデルのエンドポイントと名前を設定
porta agent examples/chat-agent/agent.toml -- "Add 20 and 22 using the tool."
```

判断ループもツールも WASM の中で動きます。モデルの資格情報はホスト側が保持し、
予算は委譲したチーム全体で共有され、ツールの引数は実行前に検証されます。
クラッシュに備えて記録する場合:

```bash
porta agent agent.toml --record run.jsonl -- "..."
porta agent-resume agent.toml run.jsonl   # 完了済みの書き込みは繰り返さない
porta agent-journal run.jsonl             # 読み取り専用、コードは読み込まない
```

## Porta が強制すること

| 懸念 | 仕組み |
|---|---|
| ワークスペース外への書き込み | `-v` によるマウント。暗黙のマウントは一切なし |
| ネットワーク送信 | OS レイヤの `--allow-net`、HTTPS CONNECT 単位の `--proxy-allow` |
| 資格情報がゲストに渡ること | モデルの資格情報はホストが保持し、エージェントには渡らない |
| エージェントが勝手に「完了」と言うこと | [完了チェック](docs/completion-checks.md): オペレータ所有の WASM。ゲストは迂回できない |
| 効果を出す前の前提条件 | [pre-tool チェック](docs/before-tool-checks.md)が書き込み・リモート呼び出し・委譲の前に走る |
| 誤った、または注入されたツール引数 | 実行前に宣言済みスキーマと照合して検証 |
| 作業途中のクラッシュ | [ジャーナル](docs/agent-journals.md): 完了済みの書き込みを繰り返さずに再開。結果が不確定な操作は決して再試行しない |
| レビュー済みのコードとの乖離 | [アーティファクトピン](docs/artifact-pins.md)が WASM と委譲ポリシーを SHA-256 に束縛 |

## 計測

公開している数値はすべて、生レポート・ソースのハッシュ・再採点する監査を伴い、
CI がその監査を実行します。Porta に不利な結果も同じ場所に公開しています。

- [起動時間とメモリ](docs/benchmarks/startup-and-memory.md) — 固定応答での計測で、
  比較の限界を明記しています。タスク品質の測定ではありません。
- [封じ込めと復旧](docs/benchmarks/containment-evaluation.md) — 敵対的入力下で、
  成功したかではなく何が外に出たかを採点。5 シナリオ中 1 つだけが差を生み、
  その封じ込めはタスク完了率を犠牲にしました。
- [実タスクの品質](docs/benchmarks/README.md) — ローカルモデルでの小規模な対照実験。
  Porta は一般的な品質優位を示せていません。共有 compute ツールの追試は、
  成果として公開せず不採用としました。

## さらに詳しく

| 項目 | ドキュメント |
|---|---|
| WASM エージェント、チーム、権限付与、上限 | [agent-runtime.md](docs/agent-runtime.md) |
| チェックポイント、再開、リプレイ | [agent-journals.md](docs/agent-journals.md) |
| エージェントからのリモート MCP ツール | [agent-mcp.md](docs/agent-mcp.md) |
| オフラインでのチーム検査 | [agent-check.md](docs/agent-check.md) |
| 完了チェック | [completion-checks.md](docs/completion-checks.md) |
| pre-tool チェック | [before-tool-checks.md](docs/before-tool-checks.md) |
| アーティファクトピン | [artifact-pins.md](docs/artifact-pins.md) |
| 制限付き計算ツール | [examples/compute](examples/compute/README.md) |
| 計測結果 | [docs/benchmarks](docs/benchmarks/README.md) |

ドキュメント本体は英語です。

## 制限事項

ネイティブの制限は **macOS** と **Linux** に対応していますが、同一ではありません。
macOS は `sandbox-exec`、Linux は Landlock を使います。どちらも書き込みと TCP
ポートを強制します。読み取りは既定ではどちらも開放ですが、`--read-policy strict`
でどちらでも閉じられます — 読み取りを、許可した mount とコマンドの起動に必要な
システムディレクトリだけに限定するので、**ホームディレクトリ配下は一切読めません**。
HTTPS プロキシのフィルタリングは macOS 限定のままです。UDP と Unix ソケットも
拒否する必要があり、Landlock ではそれを表現できないため、Linux では部分的に強制
するのではなく実行を拒否します。要求された規則をカーネルの Landlock ABI が表現
できない場合も、緩めるのではなく実行を拒否します。

ネイティブの読み取り権限は両プラットフォームとも書き込み権限より広く、コンテナの
ファイルシステムや完全な秘密情報の隔離ではありません。HTTPS プロキシの
フィルタリングは接続先を制御するもので、TLS の中身は見ません。`porta run` と
`porta serve` は `-v` を渡さない限り何もマウントしません。

## porta.toml

制限付き実行のための宣言的な設定です。

```toml
[runtime]
type = "native"           # "native" または "wasm"
command = "claude"         # 実行するコマンド (native モード)
# wasm = "agent.wasm"     # WASM バイナリ (wasm モード)

[sandbox]
mounts = ["."]            # コマンドが書き込めるディレクトリ
# mounts = [".:ro"]       # 読み取り専用マウント
network = ["*:443"]       # これらのポートに制限 (空なら全開放)

[env]
NODE_ENV = "production"

[secrets]
API_KEY = "sk-..."
# ホストの環境変数から読む場合:
# API_KEY = { from-env = true }
```

```bash
porta init native claude   # porta.toml を生成
porta up                   # porta.toml から実行
porta up -- --print "hi"   # コマンドに引数を渡す
```

## CLI リファレンス

### プロジェクト

| コマンド | 説明 |
|---------|-------------|
| `porta init [native\|wasm] [cmd]` | porta.toml を作成 |
| `porta up [-- args...]` | porta.toml から実行 |

### ランタイム

| コマンド | 説明 |
|---------|-------------|
| `porta agent <agent.toml> [--record <journal>] -- <task>` | WASM エージェントまたはチームを実行 |
| `porta agent-resume <agent.toml> <journal>` | 記録済みの実行を継続 |
| `porta agent-replay <agent.toml> <journal>` | 完了した実行をオフラインで検証 |
| `porta run <target>` | WASM (.wasm) またはネイティブコマンドを実行 |
| `porta run -d <agent.wasm>` | WASM をバックグラウンドデーモンとして実行 |
| `porta serve <agent.wasm>` | stdio で MCP サーバを起動 |

### 開発

| コマンド | 説明 |
|---------|-------------|
| `porta build <agent.wasm>` | manifest.json を生成 |
| `porta inspect <agent.wasm>` | モジュール情報を表示 |
| `porta validate <agent.wasm>` | WASI インポートをプロファイルと照合 |

### インスタンス

| コマンド | 説明 |
|---------|-------------|
| `porta ps` | インスタンス一覧 |
| `porta stop <id>` | インスタンスを停止 (SIGTERM) |
| `porta kill <id>` | インスタンスを強制終了 (SIGKILL) |
| `porta logs <id>` | インスタンスのログを表示 |
| `porta rm <id>` | 停止済みインスタンスを削除 |

### 共通オプション

| フラグ | 説明 |
|------|-------------|
| `-e`, `--env <KEY=VALUE>` | 環境変数を設定 |
| `--env-file <path>` | ファイルから環境変数を読み込み |
| `--secret <KEY=VALUE>` | シークレットを環境変数として注入 |
| `-v <path>` | ディレクトリをマウント (書き込み可) |
| `-v <path>:ro` | ディレクトリをマウント (読み取り専用) |
| `--allow-net <host:port>` | 外向き通信を許可 (繰り返し可) |
| `--allow-exec <cmd,...>` | 特定コマンドを許可 (カンマ区切り) |
| `--profile <name>` | ケイパビリティプロファイル: `ai-agent`, `worker`, `full` |
| `--step-limit <n>` | WASM の最大命令数 |
| `--max-memory <pages>` | WASM の最大メモリページ数 |
| `--restart <policy>` | `no`, `on-failure`, `always` |
| `-d`, `--detach` | バックグラウンドデーモンとして実行 |
| `--help`, `-h` | 各コマンドのヘルプを表示 |

## セキュリティモデル

### 2 層での強制

Porta は 2 つのレベルで制限を強制します。

1. **OS 層** (sandbox-exec) — プロセス単位のポートベースのネットワーク制御と
   ファイルシステム制限。子プロセスからは迂回できません。
2. **MCP 層** — アプリケーションレベルの host+port による URL フィルタリングと、
   `porta.exec` / `porta.http` 組み込みツールへのケイパビリティ検査。

### ネイティブの制限 (macOS)

`sandbox-exec` で以下を強制します。

| 制御 | 挙動 |
|---------|----------|
| **FS 書き込み** | `-v` でマウントしたディレクトリと `/tmp` 以外は拒否 |
| **FS 読み取り** | 既定では `~/.ssh` と `~/.gnupg` を拒否 (暗号鍵)、その他の読める host ファイルは引き続きアクセス可能。`--read-policy strict` で mount ＋ `/usr` `/System` `/bin` `/sbin` `/etc` `/tmp` `/dev` だけに限定 — ホームディレクトリは全て閉じます |
| **ネットワーク** | 既定は開放。`--allow-net "*:443"` で HTTPS のみに制限 |
| **読み取り専用** | `-v ./data:ro` → 読み取り可、書き込み拒否 |

> 注: macOS の sandbox-exec はポートベースのフィルタリングのみ対応します。
> ホストベースのフィルタリング (`api.example.com:443`) は、組み込みツールに対して
> MCP 層で強制されます。

### ホスト単位の HTTPS フィルタリング

```bash
porta run claude -v . --proxy-allow "api.anthropic.com,*.anthropic.com"
```

または `porta.toml` に `allow = ["api.anthropic.com"]` を持つ `[proxy]` を追加します。
`run` と `up` は同じポリシーを適用します。子プロセスはローカルの CONNECT プロキシ
にしか接続できず、直接の TCP・UDP・Unix ソケットの送信は拒否されます。対応するのは
ポート 443 の HTTPS CONNECT のみで、クライアントは `HTTPS_PROXY` を尊重する必要が
あります。これは接続先を絞るもので、TLS の中身は見ません。拒否リストは明示的な
許可リストより弱い仕組みです。資格情報ブローカーでもプライベートアドレスの
フィルタでもありません。

### ネイティブの制限 (Linux)

Landlock を使います。特権も名前空間も外部ランタイムも不要です。

| 制御 | 挙動 |
|---------|----------|
| **FS 書き込み** | `-v` でマウントしたディレクトリ、`/tmp`、`/dev` 以外は拒否 |
| **FS 読み取り** | 既定は開放。`--read-policy strict` で mount ＋ `/usr` `/lib` `/bin` `/sbin` `/etc` `/proc` `/tmp` `/dev` だけに限定 — ホームディレクトリは全て閉じます |
| **ネットワーク** | 既定は開放。`--allow-net '*:443'` で TCP connect をポート単位に制限（Landlock ABI 4 以上が必要） |
| **プロキシモード** | 拒否。UDP と Unix ソケットも塞ぐ必要があり、Landlock では表現できません |

カーネルが表現できない規則を要求された場合は、緩めるのではなく実行を拒否します。
部分的な強制を黙って受け入れることはありません。

ネイティブの読み取り権限は両プラットフォームとも書き込み権限より広く、コンテナの
ファイルシステムや完全な秘密情報の隔離ではありません。

### WASM サンドボックス

既定で拒否するケイパビリティ方式です。すべての WASI インポートは、実行前に
ケイパビリティ集合と照合されます。

| ケイパビリティ | 制御対象 |
|------------|----------|
| `io` | stdin/stdout/stderr |
| `fs` | ファイル読み取り (path_open, stat, readdir) |
| `fs.write` | ファイル書き込み (作成、リネーム、削除) |
| `process` | プロセスのライフサイクル、引数 |
| `env` | 環境変数 |
| `clock` | 時刻・クロック |
| `random` | 乱数バイト |
| `net` | ネットワークアクセス |
| `exec` | コマンド実行 |

組み込みプロファイル: `ai-agent` (IO + Process)、`worker` (+Clock +Random)、`full` (全部)。

マニフェストのケイパビリティは `serve` と `run` の両モードで尊重されます。
`porta.exec_command` / `porta.http_request` の WASM インポート直呼びは拒否され、
ホストでの実行と HTTP リクエストは検査済みの MCP 組み込みツールを経由します。
`porta.http` は userinfo のない HTTP(S) URL を受け付け、リダイレクトには追従せず、
ホストのプロキシ設定も継承しません。

## MCP サーバ

```bash
porta serve agent.wasm --profile full
```

### 組み込みツール

| ツール | 必要条件 | 説明 |
|------|----------|-------------|
| `porta.exec` | `CapExec` + `--allow-exec` | ファイルシステムとネットワークの制限下でコマンドを実行 |
| `porta.http` | `CapNet` + `--allow-net` | 許可されたホストに HTTP リクエストを送信 |
| エージェントツール | — | WASM エージェントにディスパッチ |

### 対応 MCP メソッド

`initialize`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`, `ping`

### Claude Code との連携

```json
{
  "mcpServers": {
    "agent": {
      "type": "stdio",
      "command": "porta",
      "args": ["serve", "agent.wasm", "--profile", "full", "--allow-net", "*:443"]
    }
  }
}
```

## アーキテクチャ

```
porta
├── cli.almd            — オプション、引数解析、ヘルプ
├── mod.almd            — コマンドディスパッチ (エントリポイント)
│
├── engine.almd         — serve, run, validate, inspect
├── dispatch.almd       — WASM インスタンスのライフサイクルとツールディスパッチ
├── mcp.almd            — MCP プロトコル (JSON-RPC 2.0 / stdio)
├── jsonrpc.almd        — 改行区切り JSON-RPC
├── sandbox.almd        — ケイパビリティベースのセキュリティ
│
├── ops.almd            — デーモン管理 (ps/stop/kill/logs/rm)
├── build.almd          — マニフェスト生成
├── project.almd        — porta.toml (up/init)
│
├── wasm_rt.almd        — Wasmtime ブリッジとランタイム関数
├── config.almd         — porta.toml パーサ
├── manifest.almd       — manifest.json パーサ
├── observability.almd  — 実行メトリクス
├── util.almd           — CLI ユーティリティ
│
└── wasm/
    ├── binary.almd     — WASM バイナリパーサ
    └── wasi.almd       — WASI Preview 1 ホスト関数
```

## ソースからのビルド

```bash
# ソースから (Almide 0.63.0、Rust ツールチェーン、Python 3、curl)
bash scripts/install-almide.sh
.tools/almide/almide build src/mod.almd -o target/porta
.tools/almide/almide test --ci
python3 scripts/integration.py target/porta
cp target/porta ~/.local/bin/
```

コンパイラのターゲットは **0.63.0** です。2026-09-19 時点で公開されている成果物は
`v0.63.0-rc1` (`almide 0.63.0` と表示) で、インストーラはそのリリースをピン留めし、
公開されている SHA-256 を検証します。最終タグが公開されたら
`ALMIDE_RELEASE_TAG=v0.63.0 bash scripts/install-almide.sh` で選択してください。

## 言語サポート

| ランタイム | 状態 | 例 |
|---------|--------|---------|
| Almide → WASM | 完全対応 | `porta run agent.wasm` |
| Python 3.14 | WASM 内で動作 | `porta run python.wasm -- script.py` |
| ネイティブコマンド | OS による制限 | `porta run claude -- --print "hi"` |

## ライセンス

Apache-2.0
