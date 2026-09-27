<p align="center">
  <img src="docs/assets/logo.png" alt="Porta" width="200">
</p>

<h1 align="center">Porta</h1>

<p align="center">
  コマンドも AI エージェントも、与えた権限だけで動かす。<br>
  macOS と Linux のカーネルで強制。コンテナもデーモンも要らない。
</p>

<p align="center">
  <a href="README.md">English</a> · 日本語
</p>

---

```text
$ porta run sh -v ./work --no-net -- -c 'echo ok > out.txt; cat ~/.ssh/id_ed25519; echo x > ~/elsewhere.txt; curl https://example.com'
cat: /Users/me/.ssh/id_ed25519: Operation not permitted
sh: /Users/me/elsewhere.txt: Operation not permitted
curl: (6) Could not resolve host: example.com
[porta] the sandbox refused this run 3 times; what each would have needed:
  file-read-data /Users/me/.ssh/id_ed25519
    → closed by the preset or --deny-read; no mount reopens it
  file-write-create /Users/me/elsewhere.txt
    → -v /Users/me
  …
```

`out.txt` は書けた。鍵も、外のファイルも、ネットワークも届かなかった。

Porta は 1 つのプロセスと、そこから起動されるすべてを包む。書き込みはマウントした
ディレクトリだけ、認証情報の置き場所は読めず、ネットワークは指定したポートやホストに
絞られ、CPU・メモリ・プロセス数・時間には上限がかかる。強制するのは macOS では
Seatbelt、Linux では Landlock・seccomp・namespace で、お願いベースのラッパーではない。
カーネルが表現できないルールは、弱めて通すのではなく実行を拒否する。

## インストール

```bash
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

| 環境 | 方法 |
|---|---|
| macOS (Apple silicon)、Linux (x86-64, arm64) | 上のスクリプト。SHA-256 を検証し、`cosign` があれば Sigstore 署名も検証 |
| Debian、Ubuntu | [リリース](https://github.com/almide/porta/releases)の `sudo apt install ./porta_<version>_amd64.deb`。Ubuntu 23.10 以降では namespace に必要な AppArmor プロファイルも読み込む |
| それ以外の方法で入れた Ubuntu | 一度だけ `sudo porta setup` で同じプロファイルを入れる |
| GitHub Actions | `- uses: almide/porta@v0.6.16` の後、どのステップでも `porta run …` |
| その他 (Intel Mac など) | `almide install github.com/almide/porta --branch main` でソースからビルド |

すべてのリリースは署名とビルド来歴付き —
[リリースの検証](docs/cli.md#verifying-a-release)。

## 今使っているコーディングエージェントを閉じ込める

```bash
cd my-project
porta init claude            # または: porta init codex
porta up -- -p "Fix the failing test"
```

`porta init` は、そのエージェントに必要な分だけを測った、コメント付きの `porta.toml`
を書く。プロジェクトとエージェント自身のディレクトリは書き込み可、そこにある設定・
フック・グローバル指示は書き込み不可 — 1 つのセッションが次のセッションに何かを
仕込めないように — そして API キーやトークンは名前で渡す。macOS では Keychain が
閉じているので、ログインは `ANTHROPIC_API_KEY` か `CLAUDE_CODE_OAUTH_TOKEN`
(`claude setup-token`) で渡す。

他のコマンドもフラグで同じように動く:

```bash
porta run ./installer -v ./sandbox --allow-net '*:443' --timeout 120 --max-procs 500
```

`--` より前は porta の、後ろはコマンドの引数。

## 何を止めるか

| | macOS | Linux |
|---|---|---|
| **書き込み** | `-v` のマウント、`/tmp`、`/dev` だけ | 同じ |
| **書き込み可のマウントの中** | `.git/hooks`、`.git/config`、`.envrc`、シェルの rc、エージェントの設定などは書けず、リネームで退かすこともできない | 同じ (user namespace が必要 — 下記) |
| **読み取り** | 認証情報以外すべて: `~/.ssh`、`~/.aws`、`~/.config/gh`、クラウドやレジストリのトークン、Keychain、ブラウザのプロファイルは閉じる。`--read-policy strict` で読み取りもマウント内に限定 | 同じ (Keychain を除く) |
| **認証情報のソケット** | SSH agent、gpg-agent、`docker.sock` は `--allow-unix` なしでは拒否 | 同じ |
| **ネットワーク** | 開放、ポート単位の TCP (`--allow-net`)、porta のプロキシ経由のホスト単位 (`--proxy-allow`)、遮断 (`--no-net`) | 同じ。ポート指定時は UDP も閉じる |
| **他のプロセス** | 引数や環境変数は読めない。`open(1)` / Launch Services も閉じる | 見えない: 専用の PID namespace |
| **リソース** | `--timeout`、`--max-cpu`、`--max-procs`、`--max-file-size`、`--max-memory-mb` | 同じ (メモリは cgroup v2) |
| **環境変数** | 空から始まる。`PATH`、`HOME`、ロケール、端末だけが渡り、他は `-e` / `--env-pass` で指定したものだけ | 同じ |

何を閉じるかはコードに書いたリストではなくプリセット —
[`native/presets/default.toml`](native/presets/default.toml)、`porta explain` で表示 —
で、`--deny-read`、`--protect`、`--deny-unix` で足し、`--preset <file>` で置き換える。
理由つきの全体表は [enforcement](docs/enforcement.md) に。

## 拒否されたとき

```bash
porta explain ./tool -v ./work --allow-net '*:443'   # ポリシーを文章で。何も実行しない
porta run ./tool -v ./work --why                      # 実行後: 拒否ごとに必要だったフラグ
```

```text
[porta] the sandbox refused this run 2 times; what each would have needed:
  file-write-data /home/me/notes/out.txt
    → -v /home/me/notes
  network-outbound remote:*:80
    → --allow-net '*:80'
  to run again with those granted:
    porta run ./tool -v ./work -v /home/me/notes --allow-net '*:80'
```

macOS では失敗した実行のあと、カーネルの拒否ログからこれを出す。Linux では `--why`
がサンドボックスの外から `strace` で実行を追う。

## 変更を元に戻す

```bash
porta run ./agent -v ./project --snapshot     # 先にマウントを複製し、終了後に変更点を一覧
porta rollback --yes                          # 元に戻す
```

APFS ではクローンなので、複製はほぼタダ。スナップショットはどのマウントの外にも
置くので、コマンドが自分の取り消し手段を書き換えることはできない。

## 他のツールとの比較

同じ[脱出コーパス](docs/benchmarks/escapes.md) — 実際の抜け道を、何が通ったかで採点 —
を 5 つのツールでそれぞれの既定のまま実行した
([全比較](docs/benchmarks/competitors.md)、負けた行も含む):

| | porta | [srt](https://github.com/anthropics/sandbox-runtime) | [Fence](https://github.com/fencesandbox/fence) | [nono](https://github.com/nolabs-ai/nono) | [landrun](https://github.com/Zouuup/landrun) |
|---|---|---|---|---|---|
| macOS: 試行 / 脱出 | **22 / 0** | 17 / 5 | 14 / 5 | 13 / 5 | — |
| Linux: 試行 / 脱出 | **28 / 0** | 23 / 2 | 21 / 3 | 24 / 5 | 22 / 7 |

他にあって porta にないもの: srt と Fence は既定でネットワークを閉じる。Fence は
コマンドを名前でフィルタする。nono は実行中に追加の権限を求められる。porta の
オーバーヘッドは macOS で約 16 ms、Linux で 2–6 ms
([overhead](docs/benchmarks/overhead.md))。

## 制限

- **ネットワークは既定で開いている。** `--no-net`、`--allow-net`、`--proxy-allow`
  で閉じる。
- **Linux では一部の保護に user namespace が必要** — マウント内のフック、認証情報の
  ソケット、他プロセスの隠蔽。Ubuntu 23.10 以降は `.deb` か `sudo porta setup` が
  プロファイルを入れるまで制限される。無い場合も porta は動き、何が開いたままかを言う。
- **カーネルは共有。** VM ではなくプロセスのサンドボックスなので、カーネルの脆弱性は
  対象外。何を守り何を守らないかは[脅威モデル](docs/threat-model.md)に。
- **テストは作者によるもの。** コーパス、ファザー、モンキーテストは公開され CI で
  回っているが、第三者による監査はまだない。

## その他の機能

- **アンビエント権限のない WASM。** `porta run plugin.wasm --profile worker` は
  コアモジュールや WASI 0.2/0.3 コンポーネントを、ホストのファイルシステムも
  ネットワークもなし、fuel・メモリ・時間に上限をつけて実行する。import はすべて
  付与した capability が必要。
- **ループが WASM のエージェント。** `porta agent agent.toml` は判断ループと各ツールを
  別インスタンスで動かし、モデルの認証情報はホストに置き、クラッシュした実行を
  完了済みの書き込みを繰り返さずに再開する ([agent runtime](docs/agent-runtime.md))。
- **1 つの API とログ。** `--proxy-allow api.example.com --proxy-audit egress.jsonl`
  はそのホストだけを通し、すべての判断を記録する。

## ドキュメント

- [CLI リファレンス](docs/cli.md) — すべてのコマンド、フラグ、`porta.toml` のキー、終了コード
- [Enforcement](docs/enforcement.md) — 各プラットフォームが何をどう止めるか
- [脅威モデル](docs/threat-model.md) — 何を守り、何を守らないか
- [ベンチマーク](docs/benchmarks/README.md) — 脱出コーパス、比較、オーバーヘッド
- [アーキテクチャ](docs/architecture.md) — ポリシーは Almide が決め、Rust が強制する

[Almide](https://github.com/almide/almide) と [Wasmtime](https://wasmtime.dev) で
作られている。Apache-2.0。
