# veridict

[English](README.md) | 日本語

`veridict` は、候補の採用判断に使う独立したRustライブラリとCLIです。外部で集めた評価結果を読み、
宣言した条件で候補と基準を比較し、理由とともに `pass`、`fail`、`inconclusive` を返します。
統計仕様は特定の用途に依存しません。

中心となる用途は、ペア比較、逐次検定、実行上の失敗を含む再現可能な回帰判定です。評価の実行と
採用後の操作は呼び出し側が担います。責任範囲と開発の優先順位は
[プロダクトの目的と範囲](docs/product-scope_ja.md)にまとめています。

リポジトリ内の文書は現在のソースツリーを説明します。最新の公開版は `0.19.6` です。その時点のソースと
文書は [v0.19.6タグ](https://github.com/kent-tokyo/veridict/tree/v0.19.6)を参照してください。

## インストール

```bash
cargo install veridict
```

ライブラリとして使う場合:

```bash
cargo add veridict
```

## クイックスタート

勝ち・負け・引き分けを比較する例:

```bash
veridict compare examples/winloss.jsonl \
  --metric winrate \
  --pass-above 0.02 \
  --fail-below -0.01
```

対応のある数値スコアを比較し、レポートを保存する例:

```bash
veridict compare examples/paired_scores.jsonl \
  --metric mean-diff \
  --min-effect 0.01 \
  --report-json report.json \
  --report-md report.md
```

Eloの逐次検定:

```bash
veridict sprt examples/winloss.jsonl --elo0 0 --elo1 10
```

標準入力は `-` で指定します。既定はJSONLです。`.csv` または `--format csv` でCSVを選べます。

## 入力

1レコードが1観測です。対応スコア、勝敗、ステータスを記録できます。

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
```

CSVも同じフィールドを使います。不正なJSON、必須フィールドの欠落、ペア条件を満たさないID内の
レコード数、非有限値、空入力、不正な設定は終了コード `3` です。例は [`examples/`](examples/)、形式は
[入力スキーマ](schemas/input-record.schema.json)を参照してください。

## コマンド

| コマンド | 用途 |
|---|---|
| `compare` | 固定標本による候補と基準の判定 |
| `sprt` | Wald、三項、五項の逐次検定 |
| `matrix` | Bradley-Terry/Eloによる候補間比較 |
| `plan` | 証拠を追加すべき比較の順位付け |
| `power` | データ収集前の必要証拠量の見積もり |
| `verify-run` | 実験記録の構造と整合性の検証 |
| `time-sensitive` | 判定時刻の価値を含む実験的な片側検定 |

CLIオプションの正本は `veridict <command> --help` です。統計上の前提や併用できない設定は
[`docs/metrics_ja.md`](docs/metrics_ja.md)にまとめています。

## 比較メトリクス

| メトリクス | 入力 | 効果量 |
|---|---|---|
| `winrate` | `result` | 決着時の候補勝率から `0.5` を引いた値 |
| `sign-test` | 対応スコア | 正符号率から `0.5` を引いた値 |
| `mean-diff` | 対応スコア | `candidate - baseline` の平均 |
| `quantile-diff` | 対応スコア | 対応差の指定分位点 |
| `relative-diff` | 正の基準値 | 比例変化の平均 |
| `elo` | `result` | 勝ち・引き分け・負けの得点から求めるロジスティックElo |
| `paired-elo` | IDごとにちょうど2結果 | ペア得点を保持したロジスティックElo |

ペア得点を保持する固定標本Eloの例:

```bash
veridict compare examples/chess_engine_paired_openings.jsonl \
  --metric paired-elo \
  --paired-by-id \
  --min-effect 10
```

主な契約:

- `--paired-by-id` は同じIDの最大2レコードをまとめます。`paired-elo` はより厳格で、すべてのIDに
  利用可能な結果がちょうど2件必要です。
- `--cluster-by-id` は `winrate` または `elo` をクラスター単位で再標本化します。ペア化とは別機能です。
- `--failure-policy` は勝敗への反映方法を決めます。失敗件数は常に報告し、妥当性上限の超過は
  `validity` と `reason` に記録します。
- `--metric` を繰り返すと複数項目をまとめて判定します。優先順位は
  `fail > inconclusive > pass` です。
- `--claim-correction bonferroni|holm` は、対応するメトリクスについて多重比較補正後の結果を追加します。
- 固定標本で決定的な判定を出すには、独立した分析単位が2件以上必要です。2件未満なら
  `inconclusive` になります。

## 判定と出力

信頼区間を使う場合、下限が合格しきい値以上なら `pass`、上限が不合格しきい値以下なら `fail`、
それ以外の有効な結果は `inconclusive` です。妥当性上限を超えると、`validity=invalid`、
`verdict=inconclusive`、`promotion=not_promoted` になります。

`compare` と `sprt` の終了コードは次のとおりです。

| コード | 意味 |
|---|---|
| `0` | pass |
| `1` | fail |
| `2` | inconclusive |
| `3` | 入力または設定が不正 |

`time-sensitive` は `pass` にコード `0`、`inconclusive` に `2`、入力・設定エラーに `3` を使います。
片側検定の契約上、統計的な `fail` を表すコード `1` はありません。

JSONが機械処理向けの契約で、Markdownは人向けの要約です。中間値から判定を再構成せず、
`verdict` と `promotion` を使用してください。

## GitHub Actionsから再利用する

`compare`、`sprt`、`verify-run`を呼び出せる再利用ワークフローを同梱しています。ワークフロー内で
指定したveridictの版をインストールし、呼び出し側が用意した結果だけを判定します。利用側の
シークレットは不要です。

```yaml
jobs:
  gate:
    uses: kent-tokyo/veridict/.github/workflows/veridict-gate.yml@v0.19.6
    with:
      command: compare
      input_path: results.jsonl
      arguments_json: '["--metric","winrate","--pass-above","0.02","--fail-below","-0.01"]'
```

このワークフローはveridict `0.19.6`をインストールします。本番利用では、次の点を守ってください。

- ワークフローの参照先をリリースタグまたは完全なコミットSHAに固定する。
- 有効な `compare` と `sprt` は、終了コード `0`、`1`、`2` をジョブへ反映する前に
  `report.json` と `report.md` を保存する。有効な `verify-run` はコード `0` または `1` を使う。
- 入力・設定エラーはコード `3` とし、統計レポートを作らず、`gate.json`、`gate.md`、
  `stderr.txt` を証拠として残す。
- `arguments_json` はJSON文字列配列で渡す。各引数の境界は維持され、シェルでは再評価されない。

`verify-run` のコード `1` は構造上の不備を表し、統計的な `fail` ではありません。

## ドキュメント

- [`docs/product-scope_ja.md`](docs/product-scope_ja.md): 製品の目的、責任範囲、開発の優先順位。
- [`docs/metrics_ja.md`](docs/metrics_ja.md): 実装済みの統計仕様とレポート契約。
- [`docs/performance_ja.md`](docs/performance_ja.md): ストリーミング、保持状態、ベンチマーク契約。
- [`docs/research-map_ja.md`](docs/research-map_ja.md): 未実装候補と着手条件。
- [`CHANGELOG.md`](CHANGELOG.md): リリース履歴。
- [JSON Schemas](schemas/): 記載された入出力形式の版付き契約。`time-sensitive`のレポートスキーマは
  まだありません。

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
