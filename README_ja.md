# veridict

[English](README.md) | 日本語

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

候補(candidate)がベースライン(baseline)より本当に優れているかを判定する、小さくドメイン非依存な評価ゲート。トライアル結果のファイルから判定します。

`veridict` はベンチマークランナーでも実験トラッカーでもありません。結果を受け取り、判定を返す統計的な意思決定レイヤーです:

* `pass`
* `fail`
* `inconclusive`(判定不能)

データがノイジー・少数・不明瞭な場合は、過大に主張するのではなく `inconclusive` を返します。誤ったpassは、判定不能な結果よりも悪いものです。

## ユースケース

「スプレッドシートを目で見て何となく判断していた」ような、あらゆる"candidate vs baseline"比較に使えます:

* **ゲーム/探索エンジンのリグレッション検知** - 勝敗/引き分けの対局結果 →
  `--metric winrate`、`--metric elo`、または逐次検定したいなら `veridict sprt`。
* **OCRや抽出パイプラインの精度比較** - 文書ごとの精度スコア →
  `--metric mean-diff` または `--metric sign-test`。
* **LLMプロンプト/モデルの比較** - ペアワイズのジャッジ結果や数値品質スコア →
  `--metric winrate` または `--metric mean-diff`。
* **ランキング/最適化アルゴリズムのチューニング** - 実行ごとの数値目的関数
  (NDCG、loss、スループットなど) → `--metric mean-diff`。
* **CIでのリリースリグレッションゲート** - 候補ビルドを直近の正常なベースラインと比較し、
  `--fail-below`/`--pass-above` と `veridict` の終了コードでパイプラインに組み込む
  (下記の[使い方](#使い方)のリグレッションゲート例を参照)。
* **3つ以上の案を比較** - 同じ共有ベースラインに対する複数のプロンプト/設定 →
  `veridict matrix`。

## インストール / ビルド

```bash
cargo build --release
```

## 使い方

```bash
veridict compare results.jsonl --metric winrate --min-effect 0.02 --confidence 0.95
veridict compare scores.jsonl  --metric mean-diff --min-effect 0.01 --confidence 0.95
```

非対称なしきい値によるリグレッションゲート:

```bash
veridict compare results.jsonl \
  --metric winrate \
  --fail-below -0.01 \
  --pass-above 0.02 \
  --confidence 0.95 \
  --report-json report.json \
  --report-md report.md
```

`-` で標準入力から読み込み:

```bash
cat results.jsonl | veridict compare - --metric winrate
```

同じ入力に対して複数のメトリクスを一度に実行できます。全体の判定は個々の判定のうち最も厳しいもの(`fail` が一つでもあれば全体もfail、次に `inconclusive`、それ以外は `pass`)になります:

```bash
veridict compare results.jsonl --metric winrate --metric sign-test --min-effect 0.02
```

逐次検定(sequential testing): 候補が少なくとも `--elo1` ポイント強いと確信できるまで(pass)、あるいは高々 `--elo0` ポイントの強さだと確信できるまで(fail)、もしくはデータが足りない(inconclusive)と判定されるまで、結果を投入し続けます:

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

同じ共有ベースラインに対して測定した3つ以上の候補を一度に比較し、ペアワイズのElo差を一覧表にします:

```bash
veridict matrix prompt_a.jsonl prompt_b.jsonl prompt_c.jsonl
```

### 終了コード

| コード | 意味 |
|------|---------|
| 0 | pass |
| 1 | fail |
| 2 | inconclusive(判定不能) |
| 3 | 不正な入力または設定エラー |

## 入力フォーマット

1行1レコード: デフォルトはJSONL、またはCSV(`--format csv`、または `.csv` 拡張子から自動判定)。両者は同じフィールドを共有します。`examples/` を参照してください:

* `examples/winloss.jsonl` - 勝敗/引き分けのレコード。`--metric winrate` / `--metric sign-test` 用。
* `examples/paired_scores.jsonl` - baseline/candidateのペア数値スコア。`--metric mean-diff` / `--metric sign-test` 用。
* `examples/status_failures.jsonl` - サポートされる全レコード形式をまとめてフォーマットを例示したもの(そのまま単一メトリクスに対して実行することは想定していません: レコードは選択したメトリクスが理解できるフィールド、または `baseline_status`/`candidate_status` フィールドのいずれかを持つ必要があり、なければスキーマ不一致として拒否されます)。

```json
{"id":"case-001","baseline":0.81,"candidate":0.84}
{"id":"case-002","result":"candidate_win"}
{"id":"case-003","result":"draw"}
{"id":"case-004","baseline_status":"ok","candidate_status":"timeout"}
{"id":"case-005","baseline_status":"ok","candidate_status":"invalid"}
```

CSVも同じ形で、空セルは値が存在しないフィールドとして扱われます:

```csv
id,baseline,candidate,result,baseline_status,candidate_status
case-001,0.81,0.84,,,
case-002,,,candidate_win,,
case-004,,,,ok,timeout
```

## メトリクス

* **`winrate`** - 決着がついた(引き分けを除く)`result` レコードに対するWilsonスコア区間。
* **`sign-test`** - baseline/candidateのペア数値レコードのうち、candidateがbaselineを上回った割合(タイは除外)に対するWilsonスコア区間。`mean-diff` のノンパラメトリックな代替: 差の大きさではなく方向のみに着目します。
* **`mean-diff`** - baseline/candidateのペア数値レコードに対する `candidate - baseline` のパーセンタイルブートストラップ信頼区間。`--resamples` でブートストラップのリサンプル数、`--seed` でRNGシードを制御できます(デフォルトは固定シードなので、同じ入力ならCI上でも出力がビット単位で一致します)。
* **`elo`** - 勝敗/引き分けの `result` レコードから算出するEloレーティング差(`winrate`/`sign-test` と異なり、引き分けは半勝として数えます)。標準的なロジスティックモデルでEloポイントとして報告します。

`winrate` と `sign-test` は `effect`/`ci_low`/`ci_high` を0を中心とした値(五分五分からの偏差)として報告します。`elo` も構造上0を中心とします(五分の成績は0 Elo)。この3つはいずれも `--min-effect` とそのまま組み合わせられます。`mean-diff` は入力そのものの単位で報告します。

各トライアルの `baseline_status`/`candidate_status`(`timeout`、`crash`、`invalid`)は、どのメトリクスを実行してもタリーされ、レポートに含まれます。合計値だけでなく、どちら側で失敗したかの内訳(JSONレポートの `failure_breakdown`)も出力されます。

## SPRT

`veridict sprt` は `compare` とは別のモードです。効果量としきい値と照合する信頼区間の代わりに、決着がついた(引き分けを除く)`result` レコードに対して対数尤度比(Wald古典的な二値SPRT)を累積し、`--alpha`/`--beta` から導かれる2つの境界のいずれかを超えた時点で停止します。`pass` は「候補が少なくとも `--elo1` ポイント強いと確信できる」、`fail` は「高々 `--elo0` ポイントの強さだと確信できる」、`inconclusive` は「データ収集を継続する」ことを意味します。`--alpha`/`--beta` はレポート上の調整可能なつまみではなく、実際に保証される偽陽性率/偽陰性率そのものです。このサブコマンドに `--min-effect`/`--confidence` はありません。

## 比較マトリクス(comparison matrix)

`veridict matrix` は候補ごとに1ファイルを受け取ります。いずれも*同じ共有ベースライン*に対して測定されたもので、`--metric elo`/`--metric winrate` と同じ `result` フィールドの勝敗/引き分けレコードを使います。そしてペアワイズのElo差を一覧表にします。レポート専用で(判定なし)、成功時は常に終了コード0です: マトリクス全体に対する単一のpass/failは存在しません。

各候補は常に共有ベースラインとのみ対戦し、候補同士は直接対戦しないため、背後のモデルはスターグラフ(star graph)になります。つまり各候補のレーティングはそのままその候補自身のElo-vs-baselineに一致します(スターグラフ上のBradley-TerryのMLEには共同で解くべき共有項が存在せず、反復ソルバーは不要です)。`baseline` に対するセルは直接データですが、候補同士のセルはモデルによる外挿(`elo_i - elo_j`、Markdown表では `*` を付与)であり、その信頼区間は2つの独立したサンプル間の正規近似による誤差伝播から求めています。そのため予想どおり、直接データのセルより幅が広くなります。

## ペアテストケース(paired testcases)

`--paired-by-id`(`compare`、`sprt`、`matrix` で使用可能)は、同じ `id` を持つ2つのレコードを「同じテストケースを2回実行したもの」(例: そのテストケース固有のバイアスを打ち消すために役割を入れ替えて再実行したもの)とみなし、2つの独立した観測ではなく1つの正味の観測として結合します:

* `winrate`/`elo`: ペア全体の合計ポイント(勝ち=1、引き分け=0.5、負け=0という、いわゆる「ペアゲーム」の標準的な採点方式)で正味化します - 合計が`1`より大きければ正味candidate勝ち、`1`未満なら正味baseline勝ち、ちょうど`1`なら正味引き分けです。
* `mean-diff`/`sign-test`: ペアの2つの差分の平均で正味化します。

`id` が1回しか出現しない場合は通常のペアなしサンプルとして扱われます(1つのファイルにペアありとペアなしのテストケースが混在しても問題ありません)。同じ `id` を持つレコードが3つ以上ある場合は、ペアへ黙って切り詰めるのではなく、データエラーとして拒否されます。`--paired-by-id` を指定しない場合、`mean-diff`/`sign-test` レコードの `id` 重複はこのフラグの有無に関わらず従来どおり拒否されます。

## 判定ロジック

このゲートは点推定ではなく信頼区間としきい値を比較します: `pass` は信頼区間の悲観的な(下側の)境界が `--pass-above` を上回ることを要求し、`fail` は信頼区間の楽観的な(上側の)境界が `--fail-below` 以下であることを要求します。それ以外(使用可能なトライアルが0件の場合を含む)はすべて `inconclusive` です。

`--min-effect X` は対称なしきい値(`--pass-above X --fail-below -X`)の省略形で、デフォルトは `0` です。

## 開発

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo audit
```

CI(`.github/workflows/ci.yml`)は、push・pull request毎にこの4つすべてを実行します。

## ライセンス

[Apache License, Version 2.0](LICENSE-APACHE) または [MIT license](LICENSE-MIT)
のいずれかを選択できます。
