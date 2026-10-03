# Porta Enterprise 評価版(仮称)

*これは評価版であり、製品リリースではありません。ライセンスキー、有効期限、課金、遠隔停止の仕組みはありません。価格と条件が決まる前に作る予定もありません。*

Porta は、信頼できない WebAssembly のジョブを、運用者が書いたポリシーの範囲内で実行します。

1. クライアントが HTTP API でジョブを送ります。ジョブに含めるのは、登録済みのどのモジュールを動かすか、入力、触ってよいディレクトリ、実行上限です。
2. Porta はジョブをポリシーと照合します。
3. ジョブごとに専用プロセスを立て、その中の WebAssembly で実行します。
4. 上限に達したら止めます。
5. 結果、停止理由、実行記録を返します。

仕様の正本は英語版です。このページは評価手順の要約です。

| 文書 | 内容 |
|---|---|
| [API と設定](api.md) | ポリシー、ジョブ、実行記録、HTTP エンドポイント、停止理由 |
| [脅威モデル](threat-model.md) | 信頼できないモジュールにできること・できないこと、各主張の試験 |
| [対応表](compatibility.md) | 実行を確認した環境、未確認の環境、各クラウドの制約 |
| [運用](operations.md) | 導入、削除、更新、記録、ログ、評価版の制約 |
| [配布](distribution.md) | ビルド、コンテナイメージ、同梱物のライセンス |
| [不足項目](gaps.md) | 製品化に足りないもの(優先順位と根拠つき) |

## 10 分で評価する

必要なのは Docker だけです。外部へ送信されるデータはありません。

```bash
git clone https://github.com/almide/porta && cd porta
docker build -t porta-eval .            # ソースからビルドします。初回は約 10 分
export PORTA_JOB_TOKEN=$(openssl rand -hex 24)
docker run -d --name porta-eval --read-only --tmpfs /tmp \
  -v porta-eval-state:/var/lib/porta -p 127.0.0.1:8080:8080 \
  -e PORTA_JOB_TOKEN porta-eval
python3 examples/enterprise/evaluate.py --url http://127.0.0.1:8080
```

`evaluate.py` は標準ライブラリだけで動きます。サンプルジョブを送り、期待どおりかを `PASS` / `FAIL` で表示します。確認する項目は次のとおりです。

| 確認すること | 期待される結果 |
|---|---|
| 許可された仕事 | 成功し、正しい集計結果が返る |
| ポリシーを超える要求(書き込み権限、未登録ディレクトリ、上限超過) | 実行前に拒否され、その拒否も記録される |
| 許可されていない読み書き(6 種類) | サンドボックス内で失敗する |
| 無限ループ | 時間上限、または燃料(命令数)上限で止まる |
| メモリの際限ない確保 | メモリ上限で止まる |
| 出力の垂れ流し | 出力上限で止まる |
| 実行中ジョブの取消 | すぐに止まる |
| 上の全ての実行 | 完了済みの記録が残る |

自分の目で確かめるには次を実行します。

```bash
H="Authorization: Bearer $PORTA_JOB_TOKEN"
curl -s -H "$H" http://127.0.0.1:8080/v1/policy                      # 適用中のポリシー(ラベルとハッシュ)
curl -s -H "$H" 'http://127.0.0.1:8080/v1/runs?limit=5'              # 直近の実行
curl -s -H "$H" http://127.0.0.1:8080/v1/runs/<run_id>               # 1 件の実行記録
docker exec porta-eval tail -3 /var/lib/porta/records/events.jsonl   # イベントログ
docker stop porta-eval && docker start porta-eval                    # 記録は残る。実行中だったジョブは service_shutdown で終わる
```

片付けは次のとおりです。

```bash
docker rm -f porta-eval && docker volume rm porta-eval-state && docker rmi porta-eval
```

## 実行記録に残るもの

実行記録に残す項目は次のとおりです。

- 実行 ID
- 適用したポリシーのラベルと SHA-256
- モジュール名と SHA-256
- ジョブの SHA-256
- 受付・開始・終了の時刻(UTC、ミリ秒)
- 結果(`succeeded` / `failed` / `stopped` / `refused`)と停止理由
- 適用した上限と付与した権限(名前のみ)
- 使用量(燃料、メモリ、経過時間)
- 出力のサイズとハッシュ

ログに無条件では出さないものは次のとおりです。

- トークン、環境変数の値、ジョブの入力:出しません。
- 標準出力の本文と出力ファイル:ポリシーで `retain_output = true` を指定した場合だけ保存します。

## 何が証明済みで、何が未検証か

| 項目 | 状態 |
|---|---|
| 許可した操作の成功、許可しない操作の拒否・失敗、上限による停止、取消、記録 | 実装済み。macOS 26 (arm64) と Linux 6.12 (Docker, arm64) で試験済み(84 項目) |
| ワーカーへの OS サンドボックス適用(多層防御) | 実装済み。上記 2 環境で適用を確認 |
| 停止(SIGTERM)、強制終了後の再起動、記録の保持 | 実装済み。コンテナで確認 |
| AWS ECS / Cloud Run / Azure Container Apps / Cloudflare Containers | **テンプレート作成済み/実環境未検証** |
| AgentCore Runtime | 設計メモのみ(未実装) |
| ネットワーク許可、利用者ごとの認証、TLS、改ざん検知つき記録 | **未実装** |

第三者による監査は受けていません。この評価版は「完全分離」や「安全」を主張するものではありません。
