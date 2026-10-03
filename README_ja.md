# veridict

[English](README.md) | 日本語

`veridict` は、評価結果から `pass`、`fail`、`inconclusive` を返す、用途非依存の統計ゲートです。
ベンチマークの実行や実験履歴の管理は行いません。証拠が足りない場合は、無理に合格とせず
`inconclusive` を返します。

モデル精度、抽出品質、探索性能、ランキング品質、レイテンシ、リリース回帰など、候補と基準の比較に
利用できます。

## インストール

```bash
cargo install veridict
```

ライブラリとして利用する場合:

```bash
cargo add veridict
```

## クイックスタート

勝ち・負け・引き分けを比較する例:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --pass-above 0.02 \
  --fail-below -0.01 \
  --confidence 0.95
```

対応のある数値スコアを比較する例:

```bash
veridict compare scores.jsonl --metric mean-diff --min-effect 0.01
```

JSONとMarkdownのレポートを保存する例:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --min-effect 0.02 \
  --report-json report.json \
  --report-md report.md
```

標準入力は `-` で指定します。既定はJSONLです。`.csv` 拡張子または `--format csv` でCSVを
選べます。

## 入力

1行が1観測です。対応のある数値、勝敗、ステータスのいずれかを記録します。

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
```

CSVも同じフィールドを使います。

```csv
id,baseline,candidate,result,baseline_status,candidate_status
case-001,0.81,0.84,,,
case-002,,,candidate_win,,
case-004,,,,ok,timeout
```

入力は厳密に検証します。不正なJSON、メトリクスに必要なフィールドの欠落、ペア化規則に反するID件数、
非有限値、空入力、不正な設定は終了コード `3` です。例は [`examples/`](examples/)、スキーマは
[`schemas/input-record.schema.json`](schemas/input-record.schema.json) にあります。

## メトリクス

| メトリクス | 入力 | 効果量と区間 |
|---|---|---|
| `winrate` | `result` | 決着時の候補勝率から `0.5` を引いた値。既定はWilson区間 |
| `sign-test` | 対応のある数値 | 非同値ペアの正符号率から `0.5` を引いた値 |
| `mean-diff` | 対応のある数値 | `candidate - baseline` の平均。ブートストラップ区間 |
| `quantile-diff` | 対応のある数値 | 対応差の指定分位点。ブートストラップ区間 |
| `relative-diff` | 正の `baseline` 値 | 比例変化の平均。ブートストラップ区間 |
| `elo` | `result` | 引き分けを0.5点とするロジスティックElo差 |

主なオプション:

- `--ci-method wilson|exact|jeffreys`: 二項メトリクスの区間推定。
- `--bootstrap-method percentile|basic|bca`: 対応しているブートストラップ区間。
- `--paired-by-id`: 同じIDのレコードを、メトリクスごとの規則で1観測にまとめる。1件だけのIDは
  そのまま使い、3件以上はエラー。
- `--cluster-by-id`: `winrate` または `elo` をID単位でクラスターブートストラップ。ペア化とは別機能。
- `--failure-policy report-only|exclude|loss`: 勝敗メトリクスと逐次検定で失敗を扱う方法。
- `--max-timeouts`、`--max-crashes`、`--max-invalid`: 無効な証拠の上限。
- `--claim-correction bonferroni|holm`: 複数メトリクスを同時に判定する際の補正。

`--metric` は複数指定できます。全体判定は、`fail`、`inconclusive`、`pass` の順に厳しい結果を
採用します。

```bash
veridict compare results.jsonl \
  --metric winrate \
  --metric elo \
  --min-effect winrate=0.02 \
  --min-effect elo=10 \
  --claim-correction holm
```

前提、数式、併用できないオプション、レポート項目は
[`docs/metrics_ja.md`](docs/metrics_ja.md) にまとめています。

## 逐次検定

`sprt` は入力を順番に評価し、最初に境界へ到達した時点で判定を確定します。

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

バリアント:

- `wald`（既定）: 決着した結果のみ。ロジスティックEloで仮説を指定。
- `trinomial`: 引き分けを考慮。`--belo0` と `--belo1` でBayesElo仮説を指定。
- `pentanomial`: 2試行のペア結果を利用。`--paired-by-id` が必要。

すべてのバリアントが入力順に再生します。`wald` と `trinomial` は利用可能な観測ごと、
`--paired-by-id` 指定時は正味ペアの完了ごとに評価します。`pentanomial` はペア完了ごとに評価します。
停止後のレコードは判定時点のLLRや勝敗件数を変えませんが、入力検証と失敗集計の対象にはなります。
失敗上限を超えた場合、最終判定は `inconclusive` になります。

判定時点の値は `decision_*`、利用可能件数や解析件数は `available_*`、`analyzed_*`、停止位置は
`stopping_*`、停止後の件数は `ignored_*_after_stop` に記録します。schema v1との互換性のため、
`wald` と `trinomial` の従来の `llr` と勝敗件数は入力全体を表します。判定には `verdict` を使用して
ください。

## その他のコマンド

### `matrix` と `plan`

```bash
veridict matrix --matches examples/matches_head_to_head.jsonl
veridict plan --matches examples/matches_head_to_head.jsonl --min-elo 20
```

`matrix` は候補間のElo差を推定します。`plan` は証拠を追加すべき比較を順位付けします。試行の実行や
スケジュール管理は行いません。`matrix` は、共通の基準に対する複数の候補ファイルも扱えます。

### `power`

```bash
veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
veridict power --sprt --elo0 0 --elo1 20
```

`mean-diff` では `--assume-sd` または `--pilot FILE` を指定します。`power` の結果はモデルに基づく
計画値であり、保証ではありません。

### 実行記録の検証

```bash
veridict verify-run examples/manifest.toml run.jsonl
```

`verify-run` は、ペア、順序、データ混入と、manifestの値と記録内の不透明な識別子・ハッシュ値との
整合性を調べます。成果物を開いてハッシュを再計算する機能ではなく、ベンチマークの質や候補の強さも
証明しません。

### 時間価値付き検定

```bash
veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
  --p0 0.50 --p1 0.55 --policy bellman \
  --reward hard-deadline --deadline 400
```

実験的な `time-sensitive` は、早い判定ほど価値が高いBernoulli単純仮説間の検定です。方策は
`bellman`、`edo`、`gro`、報酬は `hard-deadline`、`exponential`、またはカスタムスケジュールです。
片側検定であり、Elo SPRTとは別機能です。

## 判定と終了コード

信頼区間を使うゲートは、下限が合格しきい値以上なら `pass`、上限が不合格しきい値以下なら `fail`、
それ以外は `inconclusive` です。妥当性の上限を超えると `validity=invalid`、
`verdict=inconclusive`、`promotion=not_promoted` になります。警告や計画値は参考情報です。

`compare` と `sprt` の終了コード:

| 終了コード | 意味 |
|---|---|
| `0` | pass |
| `1` | fail |
| `2` | inconclusive |
| `3` | 入力または設定が不正 |

`verify-run` は構造違反を `1` で返します。`time-sensitive` には統計的なfailがなく、判定前に報酬の
期間が終わった場合は `2` です。

## ドキュメント

- [`docs/metrics_ja.md`](docs/metrics_ja.md): 統計仕様とレポート項目。
- [`docs/research-map_ja.md`](docs/research-map_ja.md): 未実装候補と着手条件。
- [`CHANGELOG.md`](CHANGELOG.md): リリース履歴。
- `veridict <command> --help`: 現在のCLIオプション。

## 開発

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo audit
```

## ライセンス

[Apache License, Version 2.0](LICENSE-APACHE) または [MIT license](LICENSE-MIT) のいずれかを
選択できます。
