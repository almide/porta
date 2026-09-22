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

## Porta とは

AI エージェントにコマンドを実行させることは、鍵もトークンもネットワークも、
その手の届く範囲に置くこと。Porta はコマンドを実行しつつ、OS カーネルに止め
させます —— 書き込み・読み取り・通信・ソケットを macOS は Seatbelt、Linux は
Landlock＋seccomp で強制。ラッパーでもお願いでもなく、OS の境界そのもの。
カーネルが表現できない制限は、緩めず実行を拒否します（**fail-closed**）。

主張ではなく証拠で。公開の脱獄[コーパス](docs/benchmarks/escapes.md)は
**macOS 17/17・Linux 21/21、突破ゼロ**、負けた行も残す（[脅威モデル](docs/threat-model.md)）。

```text
$ porta run sh -v ./work --read-policy strict --timeout 5 -- …
  ./work/out.txt に書けた            正規の作業は通る
  SSH秘密鍵を読む       → 拒否
  作業ディレクトリ外に書く → 拒否
  # ハングは締切で強制終了 → 終了コード 124
```

## インストール

```bash
curl -fsSL https://raw.githubusercontent.com/almide/porta/main/scripts/install.sh | bash
```

macOS（Apple silicon）と Linux（x86-64 / arm64）向けの単一バイナリ。公開済み
SHA-256 で検証してから導入します。Intel Mac やその他の環境は
[ソースからビルド](docs/cli.md#build-from-source)。`porta run` はネイティブ
コマンドでも `.wasm` でも取れるので、WASI にコンパイルできるものは何でも動きます。

## こう使う

`--` の前は porta のオプション、後ろはコマンドの引数です。

### すでに使っている AI エージェントを制限する

エージェントは依存を入れ、テストを走らせ、ファイルを編集する —— 鍵のあるマシンで。
1つのディレクトリと1つのホストだけ与え、残りは閉じる。

```bash
porta run claude --allow-net 'api.anthropic.com:443' -v ./project -e "HOME=$HOME" \
  -- --print "main.rs のバグを直して"
```

`claude` はそのまま動きますが、書けるのは `./project` の中だけ、繋がるのは
指定したホストだけ。`~/.ssh`・`~/.aws`・Keychain は読めません。Docker も
イメージも、エージェント本体の変更も不要。

### ツールに API を1本だけ許し、全試行を記録する

ある1つのエンドポイントだけ要る。egress を porta のプロキシに通し、記録を残す。

```bash
porta run ./agent --proxy-allow 'api.example.com' --proxy-audit egress.jsonl -v ./work
```

届くのは `api.example.com` だけ。直接 TCP・UDP・Unix ソケットの egress は拒否、
クラウドメタデータやリンクローカルに解決される名前も拒否、決定は全部
`egress.jsonl` に残ります。

### 素性の知れないコマンドを、マシンを預けずに試す

実行前にポリシーを確認してから、隔離して、締切と資源上限付きで走らせる。上限は
カーネルが、起動した子プロセス全部に強制します。

```bash
porta explain ./sketchy-installer -v ./sandbox --allow-net github.com:443   # ポリシー確認、実行はしない
porta run     ./sketchy-installer -v ./sandbox --allow-net github.com:443 \
  --timeout 60 --max-cpu 30 --max-procs 500 --max-file-size 200
```

ハングは締切で強制終了、CPU 焼きは SIGXCPU で終わり、fork 爆弾は fork できず、
ファイルは上限で止まる。systemd のユーザーセッションがある Linux では
`--max-memory-mb 512` で実行全体の実メモリを cgroup v2 で縛れます。置けない環境
では、無しで走らせるのではなくフラグを拒否します。

### 信頼できない/生成された WASM を動かす

ユーザー投稿やモデル生成のコードを、ホストの FS もネットワークもなし、燃料・
メモリ・時間を縛って実行。core module でも WASI 0.2 / 0.3 コンポーネントでも同じ検査:
宣言された import ごとに、与えた能力が要ります。

```bash
porta run plugin.wasm --profile worker --step-limit 5000000 --max-memory 256
```

### 判断ループが WASM のエージェントを作る

`porta run` は他人が書いたエージェントを制限します。`porta agent` はループ自体が
WASM のものを動かす —— ループも各ツールも別インスタンスで、環境もディレクトリも
継承せず、モデルの資格情報はホストに留まり、クラッシュしても完了済みの書き込みを
繰り返さずに再開します。

```bash
porta agent agent.toml --record run.jsonl -- "ツールを使って 20 と 22 を足して"
porta agent-resume agent.toml run.jsonl    # 完了済みの書き込みは繰り返さない
porta agent-journal run.jsonl              # 読み取り専用メタデータ、コードは読み込まない
```

詳しくは[作るエージェント](docs/agent-runtime.md)、ゲストが回避できない
[完了チェック](docs/completion-checks.md)、WASM をレビュー済み SHA-256 に縛る
[アーティファクトピン](docs/artifact-pins.md)。

### 設定をプロジェクト設定として残す

```bash
porta init native claude                 # porta.toml を生成
porta up -- --print "main.rs のバグを直して"
```

うまくいった起動をそのまま書き出すこともできます: `porta explain claude … --save porta.toml`。

## 制限が実際に止める様子を見る

効いている制限は目に見えないので、失敗するところを見ます。

```bash
porta run curl -- https://example.com                     # 既定ではネットワークは開いている
porta run curl --allow-net '*:443' -- https://example.com # HTTPS 許可 → 通る
porta run curl --allow-net '*:80'  -- https://example.com # → 終了 7、443 は拒否
```

拒否された実行は当て推量を残しません。非ゼロ終了のあと、porta はカーネルの拒否
記録を読み、どのフラグが要ったかを出します（現状 macOS。Linux は Landlock
ABI 7 が必要）。終了コードで「失敗したコマンド」と「そもそも走らなかった」を
区別できます —— [CLI リファレンス](docs/cli.md#when-a-run-is-refused)。

## 証拠

公開する数値は生レポート・入力ハッシュ・再採点の監査を必ず残し、CI がその監査を
回します。Porta に有利でない結果も同じ場所に公開します。

- [脱獄コーパス](docs/benchmarks/escapes.md) —— 既知の脱獄をバイナリに当て、
  何が抜けたかで採点。負けた行も掲載。
- [起動とメモリ](docs/benchmarks/startup-and-memory.md) —— 比較の限界を明記した
  固定応答の計測。
- [封じ込めと回復](docs/benchmarks/containment-evaluation.md) —— 敵対的入力下。
  5シナリオ中1つだけが差を分け、封じ込めはタスク完了を犠牲にした。
- [実タスク品質](docs/benchmarks/README.md) —— 一般的な品質優位は示せておらず、
  共有計算の追試は「勝ち」として公開せず却下した。

## ドキュメント

- [CLI リファレンス](docs/cli.md) —— 全コマンド・フラグ・`porta.toml` キー・終了コード。
- [強制の中身](docs/enforcement.md) —— macOS と Linux が実際に何を止めるか。
- [脅威モデル](docs/threat-model.md) —— 何を守り、何を守らないか。
- [アーキテクチャ](docs/architecture.md) —— ポリシーは Almide、強制は Rust。

## ライセンス

Apache-2.0
