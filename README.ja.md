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
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

macOS (Apple silicon) と Linux (x86-64 / arm64) 向けの単一バイナリです。
公開された SHA-256 と照合してからインストールします。バージョンは
`PORTA_RELEASE_TAG`、インストール先は第1引数で指定できます。

**Intel Mac はソースからビルドしてください。** porta は、どのマシンでも実行
されていないバイナリを公開しません。GitHub の Intel macOS ランナーが確保
できず、ビルドして検証することができませんでした。

公開されるバイナリは、**それをビルドしたマシン上で integration スイートを
通過したそのファイル**です。リリースワークフローは、これから公開するファイル
そのものに対してスイートを回します。再ビルドしたものではありません。

他のプラットフォーム、または自分でビルドする場合は
[ソースからのビルド](#ソースからのビルド)を参照してください。Almide 0.63.0、
Rust ツールチェーン、Python 3、curl が必要です。

`porta run` はネイティブコマンドと `.wasm` モジュールのどちらも取ります。WASI に
コンパイルできるものなら動きます — Almide や、`python.wasm` 経由の Python 3.14 など。

## クイックスタート

### 1. 今使っているエージェントを制限する

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -e "HOME=$HOME" \
  -- --print "Fix the bug in main.rs"
```

`claude` は通常どおり動きますが、`./project` の中にしか書き込めず、指定した
ホストにしか接続できません。Docker デーモンもコンテナイメージも不要で、
エージェント側の変更も要りません。`--` より前は porta のオプション、後ろは
コマンドへの引数です。置き場所を間違えた引数は、正しいコマンドラインを添えて
拒否されます。黙って捨てられることはありません。

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

### 4. 判断ループが WASM のエージェントを動かす

```bash
.tools/almide/almide build examples/chat-agent/src/mod.almd --target wasm -o examples/chat-agent/agent.wasm
# examples/chat-agent/agent.toml にモデルのエンドポイントと名前を設定
porta agent examples/chat-agent/agent.toml -- "Add 20 and 22 using the tool."
```

これが何なのかは[自分で作るエージェント](#自分で作るエージェント)にあります。

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

## 自分で作るエージェント

`porta run` は誰かが書いたエージェントを制限します。`porta agent` は、判断ループ
自体が WASM のエージェントを動かします。この README のほかの保証は、そこから
来ています。

```toml
# agent.toml
version = 1

[agent]
wasm = "agent.wasm"
instruction = "Use the available tools to answer the request."

[model]
endpoint = "https://api.example.com/v1/chat/completions"
name = "your-model"
token_env = "MODEL_TOKEN"          # ホストが読む。ゲストには渡さない

[limits]
max_model_calls = 16
max_steps = 64
timeout_seconds = 120

[[tools]]
name = "write_file"
wasm = "tools/write.wasm"
description = "Write text to a file in the workspace."
sha256 = "<64 hex characters>"     # レビューしたこのバイト列でなければ実行しない
mounts = [{ host = "workspace", guest = ".", read_only = false }]
input_schema = { type = "object", properties = { path = { type = "string" }, content = { type = "string" } }, required = ["path", "content"] }
```

ループもツールも別々の WASM インスタンスで動きます。どちらもあなたの環境変数も
ディレクトリも受け継ぎません。資格情報はホストに残るので、自分の設定を出力しろと
言いくるめられたエージェントには、出力するものがありません。

[Porta が強制すること](#porta-が強制すること)の各項目に、その仕組みへのリンクが
あります。あの表に無いものを3つ:

| やりたいこと | 読むもの |
|---|---|
| チーム、委譲、予算の共有 | [agent-runtime.md](docs/agent-runtime.md) |
| リモート MCP ツールを明示的に許可 | [agent-mcp.md](docs/agent-mcp.md) |
| 実行せずにチームを検査 | [agent-check.md](docs/agent-check.md) |

```bash
porta agent agent.toml --record run.jsonl -- "..."
porta agent-resume agent.toml run.jsonl   # 完了済みの書き込みは繰り返さない
porta agent-journal run.jsonl             # 読み取り専用のメタデータ、コードは読まない
```

## 制限事項

Porta が**やらないこと**を、後から気づくのではなく先に書いておきます。

- **コンテナでも VM でもありません。** 制限はあなたのカーネル上のプロセスに
  適用されます。カーネルに穴があれば、それは両者が共有している穴です。
- **読み取り権限は書き込み権限より広い**（`--read-policy strict` を渡さない限り）。
  渡した場合でも、コマンドの起動に必要なシステムディレクトリは読めます。完全な
  秘密情報の隔離ではありません。
- **プロキシのフィルタリングは接続先を絞るもので、TLS の中身は見ません。**
  子プロセスがポートで待ち受けることも止めません。
- **macOS と Linux のみ**で、しかも同一ではありません。どこが違うかは
  [ネイティブの制限](#ネイティブの制限)にあります。それ以外の環境では、
  制限なしで動くのではなく実行に失敗します。
- **暗黙のマウントはありません。** `porta run` と `porta serve` は `-v` を
  渡すまで一切のディレクトリを見ません。これは役に立つ方向の制限です。

プラットフォームが表現できない規則は、緩めるのではなく実行を拒否します。
この README の残りは、その原則に沿って書かれています。

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
| `porta explain <command> [options]` | 同じオプションで `run` が適用するポリシーを表示する。何も実行しない |
| `porta check` | このホストで強制できるもの、porta がここで拒否するものを表示する |
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
| `--allow-net <host:port>` | 外向き TCP をポート単位で許可 (繰り返し可)。ホスト名は OS 層では効きません — それは `--proxy-allow` の仕事です |
| `--proxy-allow <hosts>` | porta の CONNECT プロキシ経由にし、これらのホストだけ許可 |
| `--proxy-deny <hosts>` | 同様に、これらのホストを拒否 |
| `--proxy-audit <path>` | プロキシの判断を JSONL に追記 |
| `--read-policy <open\|strict>` | `strict` で読み取りを mount とシステムディレクトリだけに限定 (既定は `open`) |
| `--allow-root` | root でも実行する。既定は拒否 — root にとって、このポリシーが頼るファイルパーミッションは何も隔てません |
| `--env-pass <NAME,...>` | ホストの環境変数をコマンドへコピーする。子の環境は空から始まり、`PATH` `HOME` `USER` `SHELL` `TERM` とロケールだけが渡る。それ以外は `-e` かこのオプションで名指ししない限り渡らない |
| `--allow-unix <path>` | この Unix ソケットへの接続を許す。SSH エージェント、gpg-agent、コンテナランタイムのソケットは既定で閉じている（複数指定可） |
| `--allow-bind <port>` | この TCP ポートで listen することを許す。`--allow-net` が効いている間、許可したポートは「届く先」であって「待ち受ける先」ではない（複数指定可） |
| `--timeout <secs>` | この秒数を超えたらコマンドと配下のプロセスをまとめて kill し、終了コード 124 を返す。`0`（既定）は無制限 |
| `--allow-exec <cmd,...>` | 特定コマンドを許可 (カンマ区切り) |
| `--profile <name>` | ケイパビリティプロファイル: `ai-agent`, `worker`, `full` |
| `--step-limit <n>` | WASM の最大命令数 |
| `--max-memory <pages>` | WASM の最大メモリページ数 |
| `--restart <policy>` | `no`, `on-failure`, `always` |
| `-d`, `--detach` | バックグラウンドデーモンとして実行 |
| `--help`, `-h` | 各コマンドのヘルプを表示 |

### 実行が拒否されたとき

拒否は、誰かが「どのフラグがあれば通ったか」を言うまで、壊れたツールと見分けが
つきません（`Operation not permitted`）。終了コードが 0 以外だった実行の後、porta は
そのランのカーネル拒否記録を読み（macOS。deny ルールにランごとのタグが付くので、
他プロセスの拒否は混ざらない）、こう言います:

```
[porta] the sandbox refused this run 2 times; what each would have needed:
  file-write-create /Users/me/notes/out.txt
    → -v /Users/me/notes
  network-outbound remote:*:443
    → --allow-net '*:443'
```

フラグでは開かないもの（認証情報の置き場、他プロセスの引数、`open(1)`）は、
そう書かれます。`PORTA_DENIALS=always` で終了コード 0 のランでも問い合わせ、
`PORTA_DENIALS=never` で出さなくなります。Linux ではまだ出ません。Landlock ABI 7 の
監査記録が必要です。

終了コードでスクリプトは何が起きたか分かります:

| 終了コード | 意味 |
|---|---|
| コマンド自身のもの | コマンドは走った。これがその戻り値（シグナルで終わったら 128 + シグナル番号） |
| 125 | porta が開始前に拒否した — このカーネルで表現できないルール、無いマウント、`--allow-root` 無しの root |
| 126 | コマンドは存在するが、ポリシーの下では起動できない（strict の読み取り集合の外にあるインタプリタなど） |
| 127 | コマンドが見つからない |

`porta explain <command> [同じオプション]` は実行せずにポリシーを表示し、
`porta check` はこのホストで何が強制できるかを表示します。

## セキュリティモデル

### 2 層での強制

Porta は 2 つのレベルで制限を強制します。

1. **OS 層** — macOS は `sandbox-exec`、Linux は Landlock と seccomp フィルタ。
   子プロセスが起動する前に適用されるので、子プロセス側からは外せません。
2. **MCP 層** — アプリケーションレベルの host+port による URL フィルタリングと、
   `porta.exec` / `porta.http` 組み込みツールへのケイパビリティ検査。

### ネイティブの制限

表は1つにしました。2つのプラットフォームは違いますが、ほぼ同じ表を2つ読んでも
どこが違うのかは分かりません。

| 制御 | macOS (`sandbox-exec`) | Linux (Landlock + seccomp) |
|---|---|---|
| **書き込み** | `-v` マウントと `/tmp`、`/dev` 以外は拒否 | `-v` マウントと `/tmp`、`/dev` 以外は拒否 |
| **書き込み可能マウントの内側** | 既存リポジトリの `.git/hooks` と `.git/config`、ルート直下のシェル rc・`.gitconfig`・`.mcp.json`・`.npmrc`・`.claude/commands`・`.claude/agents`・`.vscode`・`.idea`・`porta.toml` は書けない。マウントルートとこれらは rename で退避できない | まだ無い（Landlock はディレクトリを丸ごと許可する） |
| **読み取り、既定** | 認証情報の置き場を拒否 — `~/.ssh` `~/.gnupg` `~/.aws` `~/.config/gcloud` `~/.docker` `~/.kube` `~/.netrc` `~/.npmrc` `~/.pypirc`、Keychain、ブラウザのプロファイル。それ以外は読める | 制限なし |
| **読み取り、`--read-policy strict`** | mount ＋ `/usr` `/System` `/bin` `/sbin` `/etc` `/tmp` `/dev` | mount ＋ `/usr` `/lib` `/bin` `/sbin` `/tmp` `/dev`、`/etc` はコマンドの起動に要るファイルだけ（ローダキャッシュ、リゾルバ、トラストストア、`passwd`、`localtime`…）。`shadow`・`sudoers`・ホスト鍵は決して含まず、一覧も取れない |
| **環境変数** | 空 ＋ `PATH` `HOME` `USER` `LOGNAME` `SHELL` `TERM` `COLORTERM` `LANG` `LANGUAGE` `LC_*` `TZ`、`-e` と `--env-pass` | 同じ |
| **他プロセス** | 引数と環境は読めない（`procargs`、`proc_pidinfo`）。シグナルは制限なし | `strict` で `/proc` は閉じる。Landlock ABI 6 ではシグナルと抽象ソケットがサンドボックス内に限定される |
| **ホストの機能** | Keychain、`open(1)`/Launch Services、マウント、ディスクとパケットデバイス、Apple Events、ネットワーク共有エージェントを閉じる | `ptrace` `process_vm_*` `pidfd_getfd` `mount*` `unshare`/`setns`/`clone(CLONE_NEW*)` `bpf` `perf_event_open` `userfaultfd` `keyctl` `io_uring` `clone3` `execveat(AT_EMPTY_PATH)` カーネルモジュール `TIOCSTI` を全モードで seccomp が拒否 |
| **読み取り専用マウント** | `-v ./data:ro` → 読める、書けない | 同じ |
| **ポート単位のネットワーク** | `--allow-net '*:443'` | 同じ。Landlock ABI 4 以上が必要 |
| **ホスト単位のネットワーク** | `--proxy-allow` のみ。`--allow-net` では不可 | 同じ |
| **プロキシモード** | プロファイルが強制 | Landlock が TCP ポートを、seccomp が残りを強制 |

`strict` ではホームディレクトリは全て閉じます。**コマンド自身もその対象**で、
これらの外に置かれたコマンドは起動できません。`/opt` 配下のツールチェーンなら
インストールディレクトリ全体を渡す必要がありますが、**読み取り専用の形を使って
ください**。`-v /opt/toolchain:ro` ならインタプリタは動いたままですが、
`-v /opt/toolchain` はエージェントにツールチェーン自体の書き換えを許します。
porta は素の `Permission denied` ではなく、どの grant が足りないかを名指しします。

Linux の Landlock は非特権で、名前空間も外部ランタイムも使いません。実行中の
カーネルが表現できない規則を要求された場合は、緩めるのではなく実行を拒否します。
部分的な強制を黙って受け入れることはありません。

### ホスト単位の HTTPS フィルタリング

```bash
porta run claude -v . --proxy-allow "api.anthropic.com,*.anthropic.com"
```

または `porta.toml` に `[proxy]` と `allow = ["api.anthropic.com"]` を書きます。
`run` と `up` は同じポリシーを適用します。子プロセスはローカルの CONNECT
プロキシにしか到達できず、直接の TCP・UDP・Unix ソケットによる送信は拒否されます。
Linux では seccomp フィルタがそれを行い、`io_uring` も拒否します — リングは
`socket(2)` を一度も呼ばずにソケットを作れるからです。

対応は HTTPS CONNECT のポート 443 のみで、クライアントが `HTTPS_PROXY` を
尊重する必要があります。拒否リストは明示的な許可リストより弱い仕組みで、
資格情報のブローカでもプライベートアドレスのフィルタでもありません。
[制限事項](#制限事項)も参照してください。

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

2つの側面があります。Almide がポリシーを決め、Rust がそれを適用してカーネルと
話します。

**`src/` — Almide.** CLI、MCP プロトコル、ケイパビリティ検査、そして実行に何を
許すかの判断。

| | |
|---|---|
| `mod.almd`, `cli.almd`, `help.almd` | コマンド振り分け、オプション、ヘルプ |
| `engine.almd`, `dispatch.almd` | serve / run / validate / inspect、WASM インスタンスのライフサイクル |
| `mcp.almd`, `mcp_builtins.almd`, `mcp_content.almd`, `jsonrpc.almd` | MCP セッション、`porta.exec` と `porta.http`、resources と prompts、フレーミング |
| `sandbox.almd`, `wasm_imports.almd` | ケイパビリティ集合と、それが照合するインポートの形 |
| `agent.almd`, `proxy.almd` | WASM エージェントとチーム、CONNECT プロキシの設定 |
| `config.almd`, `manifest.almd`, `project.almd`, `build.almd` | porta.toml、manifest.json、`init` / `up` |
| `ops.almd`, `observability.almd`, `util.almd` | デーモン、メトリクス、補助 |
| `wasm_rt.almd` | Rust 側への `@extern` すべて |

**`native/` — Rust.** Wasmtime、OS による強制、モデル資格情報を保持するブローカ。

| | |
|---|---|
| `wasmtime_bridge.rs` | WASM インスタンスのライフサイクルと FFI 面 |
| `sandbox_exec.rs`, `sandbox_profile.rs`, `landlock_policy.rs`, `landlock.rs`, `seccomp.rs` | 1つのサンドボックス要求、macOS プロファイル、Linux ruleset、Landlock が届かない出口 |
| `http_proxy.rs`, `proxy_audit.rs` | ループバック CONNECT プロキシと判断の記録 |
| `agent_runtime.rs`, `agent_journal.rs`, `agent_mcp.rs` | ブローカ、永続的な実行記録、明示的に許可されたリモート MCP 呼び出し |
| `http_client.rs`, `host_process.rs`, `wasm_inspect.rs` | 検査済みの HTTP リクエスト、プロセス補助、モジュール検査 |

porta は自前の WASM パーサを持ちません。モジュールは、それを実行するエンジンを
通して読みます。そのため `serve` と `validate` と `inspect` が同じモジュールに
ついて食い違うことはありません。

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

## ライセンス

Apache-2.0
