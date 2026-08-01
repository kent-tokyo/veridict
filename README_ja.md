# veridict

[English](README.md) | 日本語

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
[![docs.rs](https://img.shields.io/docsrs/veridict)](https://docs.rs/veridict)
[![Downloads](https://img.shields.io/crates/d/veridict.svg)](https://crates.io/crates/veridict)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![GitHub stars](https://img.shields.io/github/stars/kent-tokyo/veridict.svg?style=social)](https://github.com/kent-tokyo/veridict)

候補(candidate)がベースライン(baseline)より本当に優れているかを判定する、小さくドメイン非依存な評価ゲート。トライアル結果のファイルから判定します。

`veridict` はベンチマークランナーでも実験トラッカーでもありません。結果を受け取り、判定を返す統計的な意思決定レイヤーです:

* `pass`
* `fail`
* `inconclusive`(判定不能)

データがノイジー・少数・不明瞭な場合は、過大に主張するのではなく `inconclusive` を返します。誤ったpassは、判定不能な結果よりも悪いものです。

## ユースケース

「スプレッドシートを目で見て何となく判断していた」ような、あらゆる"candidate vs baseline"比較に使えます:

* **ゲーム/探索エンジンのリグレッション検知** - 勝敗/引き分けの対局結果 →
  `--metric winrate`、`--metric elo`、または逐次検定したいなら `veridict sprt`
  (`examples/chess_engine_winloss.jsonl`)。
* **OCRや抽出パイプラインの精度比較** - 文書ごとの精度スコア →
  `--metric mean-diff` または `--metric sign-test`
  (`examples/ocr_accuracy_paired.jsonl`、`examples/extraction_quality_paired.jsonl`)。
* **LLMプロンプト/モデルの比較** - ペアワイズのジャッジ結果や数値品質スコア →
  `--metric winrate` または `--metric mean-diff`
  (`examples/llm_prompt_ab.jsonl`)。
* **ランキング/最適化アルゴリズムのチューニング** - 実行ごとの数値目的関数
  (NDCG、loss、スループットなど) → `--metric mean-diff`
  (目的関数自体が勝敗/引き分け形式なら `examples/ranking_elo.jsonl`)。
* **レイテンシ/テール性能のリグレッションゲート** - 平均だけでは悪化した最悪ケースを見逃す →
  ペアのリクエストごとのレイテンシに `--metric quantile-diff --quantile 0.95`(または `0.99`)
  (`examples/paired_scores.jsonl`)。
* **ケースごとに問題規模が大きく異なるベンチマークスイート** - 生スコアの差は最大規模のケースに
  支配されてしまう → 絶対量ではなく比例変化を見る `--metric relative-diff`
  (`examples/mixed_scale_scores.jsonl`)。
* **CIでのリリースリグレッションゲート** - 候補ビルドを直近の正常なベースラインと比較し、
  `--fail-below`/`--pass-above` と `veridict` の終了コードでパイプラインに組み込む
  (下記の[使い方](#使い方)のリグレッションゲート例を参照)。
* **3つ以上の案を比較** - 同じ共有ベースラインに対する複数のプロンプト/設定 →
  `veridict matrix`。
* **締切や、時間とともに減衰する価値がある場合** - 早期の決定的な結果ほど価値が高い、
  固定の計算/時間予算のもとでの検定 → `veridict time-sensitive`(Bernoulliの
  simple-vs-simpleのみ。[時間価値付き検定](#時間価値付き検定time-sensitive-testing)参照)。

## インストール / ビルド

CLIとしてcrates.ioからインストール:

```bash
cargo install veridict
```

ライブラリとして依存関係に追加:

```bash
cargo add veridict
```

またはソースからビルド:

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

各メトリクス自身の主張を、同時に成立した家族として読んだときの偶然のpassから守る
([多重比較補正](#多重比較補正)を参照):

```bash
veridict compare results.jsonl --metric winrate --metric elo --min-effect 0.02 --claim-correction holm
```

小さいサンプルには正確二項信頼区間、歪んだ分布にはBCaブートストラップ:

```bash
veridict compare results.jsonl --metric winrate --ci-method exact
veridict compare scores.jsonl --metric mean-diff --bootstrap-method bca
```

問題規模や生スコアがケースごとに大きく異なるベンチマークで、baselineに対する比例変化を見る
([スケール不整合の診断](#スケール不整合の診断)参照):

```bash
veridict compare examples/mixed_scale_scores.jsonl --metric relative-diff --min-effect 0.03
```

非対称なしきい値も他のメトリクスと全く同じように使えます:

```bash
veridict compare examples/mixed_scale_scores.jsonl \
  --metric relative-diff \
  --pass-above 0.03 \
  --fail-below -0.02
```

candidateのクラッシュ/タイムアウトを、単に報告するだけでなく敗北として扱う(正確な
`report-only`/`exclude`/`loss` の意味は[メトリクス](#メトリクス)を参照):

```bash
veridict compare examples/chess_engine_with_crashes.jsonl --metric winrate --failure-policy loss
```

逐次検定(sequential testing): 候補が少なくとも `--elo1` ポイント強いと確信できるまで(pass)、あるいは高々 `--elo0` ポイントの強さだと確信できるまで(fail)、もしくはデータが足りない(inconclusive)と判定されるまで、結果を投入し続けます:

```bash
veridict sprt results.jsonl --elo0 0 --elo1 10 --alpha 0.05 --beta 0.05
```

引き分けの多いデータ(チェスエンジンのテストなど)では、trinomialバリアントが引き分けを捨てる
代わりに引き分け率を推定することで、より速く収束します:

```bash
veridict sprt examples/chess_engine_draw_heavy.jsonl --sprt-variant trinomial --belo0 0 --belo1 30
```

ペア対局形式のテスト設計(同じ開始局面を先後入れ替えで2局)では、pentanomialバリアントがペアの
2局を単一の勝敗/引き分けに正味化する代わりに、5値の合計スコアをそのまま使います。
`--paired-by-id` が必須です:

```bash
veridict sprt examples/chess_engine_paired_openings.jsonl --sprt-variant pentanomial --elo0 0 --elo1 20 --paired-by-id
```

同じ共有ベースラインに対して測定した3つ以上の候補を一度に比較し、ペアワイズのElo差を一覧表にします:

```bash
veridict matrix prompt_a.jsonl prompt_b.jsonl prompt_c.jsonl
```

または、共有ベースラインなしで、直接対戦データから名前付き対戦相手をランク付けします:

```bash
veridict matrix --matches examples/matches_head_to_head.jsonl
```

これらのペアのうち、追加トライアルによって最も不確実性が減るものを、不確実性が高い順に推薦します
(`matrix`と同じ入力に、必須の`--min-elo`を追加):

```bash
veridict plan --matches examples/matches_head_to_head.jsonl --min-elo 20
```

実際に`compare`を実行する前に、何トライアル必要かを見積もります:

```bash
veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
```

`--metric mean-diff` の場合は、想定標準偏差を直接指定するか、実際のパイロットデータから
推定します:

```bash
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --pilot examples/pilot_scores.jsonl
```

または、各仮説の下でのSPRTの期待サンプルサイズを直接見積もります:

```bash
veridict power --sprt --elo0 0 --elo1 20
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
* `examples/paired_scores.jsonl`(同じデータのCSV版が `examples/paired_scores.csv`、下記参照) -
  baseline/candidateのペア数値スコア。`--metric mean-diff` / `--metric sign-test` 用。
* `examples/mixed_scale_scores.jsonl` - baselineが広い範囲にわたり、かつすべて厳密に正である
  baseline/candidateのペア数値スコア。`--metric relative-diff` 用(および `--metric mean-diff` の
  [スケール不整合の診断](#スケール不整合の診断)のデモ用)。
* `examples/status_failures.jsonl` - サポートされる全レコード形式をまとめてフォーマットを例示したもの(そのまま単一メトリクスに対して実行することは想定していません: レコードは選択したメトリクスが理解できるフィールド、または `baseline_status`/`candidate_status` フィールドのいずれかを持つ必要があり、なければスキーマ不一致として拒否されます)。
* `examples/chess_engine_draw_heavy.jsonl` - 引き分け率の高い勝敗/引き分けレコード。
  `veridict sprt --sprt-variant trinomial` 用([SPRT](#sprt)参照)。
* `examples/chess_engine_paired_openings.jsonl` - 各 `id` がちょうど2回ずつ出現(同じ開始局面を
  先後入れ替え)。`veridict sprt --sprt-variant pentanomial --paired-by-id` 用([SPRT](#sprt)参照)。
* `examples/chess_engine_with_crashes.jsonl` - 勝敗/引き分けレコードにcandidate側の失敗を数件
  混ぜたもの。`--failure-policy loss` 用([メトリクス](#メトリクス)参照)。

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

```bash
veridict compare examples/paired_scores.csv --format csv --metric mean-diff
```

## メトリクス

* **`winrate`** - 決着がついた(引き分けを除く)`result` レコードに対する信頼区間。`--ci-method wilson`(デフォルト)、`--ci-method exact`(Clopper-Pearson、正確二項信頼区間 - どんなサンプルサイズでも被覆確率が正確ですが、常にWilsonと同じかそれ以上に幅が広くなります)、または `--ci-method jeffreys`(非情報的Jeffreys事前分布を使ったベイズ信用区間 - 多くの `p` ではWilsonとClopper-Pearsonの中間の幅になりますが、境界付近(全勝/全敗近く)ではその両方より狭くなることがあります)。`exact`/`jeffreys` はどちらも真の整数カウントの二項分布を前提とする、同じ理由による同じ制約です。
* **`sign-test`** - baseline/candidateのペア数値レコードのうち、candidateがbaselineを上回った割合(タイは除外)に対する同じ信頼区間。`mean-diff` のノンパラメトリックな代替: 差の大きさではなく方向のみに着目します。こちらも `--ci-method` を指定できます。
* **`mean-diff`** - baseline/candidateのペア数値レコードに対する `candidate - baseline` のブートストラップ信頼区間。`--bootstrap-method percentile`(デフォルト)、`--bootstrap-method basic`(percentile区間を点推定の周りで反転させる方式 - BCaより単純ですが、それ自体にバイアス補正はありません)、または `--bootstrap-method bca`(バイアス補正・加速ブートストラップ - 歪んだ差分分布を補正します。既存のCI値が黙って変わらないよう、デフォルトは引き続き `percentile` です)。`--resamples` でブートストラップのリサンプル数、`--seed` でRNGシードを制御できます(デフォルトは固定シードなので、同じ入力ならCI上でも出力がビット単位で一致します)。
* **`elo`** - 勝敗/引き分けの `result` レコードから算出するEloレーティング差(`winrate`/`sign-test` と異なり、引き分けは半勝として数えます)。標準的なロジスティックモデルでEloポイントとして報告します。`--ci-method exact` は非対応です: 勝率が小数(引き分けは半勝)になるため、Clopper-Pearsonの被覆保証が前提とする整数カウントの二項分布に当てはまりません。
* **`quantile-diff`** - baseline/candidateのペア数値レコードに対する `candidate - baseline` の
  `--quantile Q`(デフォルト `0.5`、中央値。例えばp95なら `0.95`)分位点のブートストラップ信頼区間 -
  `mean-diff` を平均から任意の分位点へ一般化したもので、平均より「典型的な最悪ケース」が重要な
  ゲート(レイテンシのp95/p99回帰ゲートなど)向けです。`--resamples`/`--seed` は `mean-diff` と
  同じ。`--bootstrap-method percentile`/`basic` のみ対応(`bca` は非対応 - 標本分位点は非平滑な
  統計量であり、BCaのジャックナイフ加速度がそれに対して堅固な裏付けを持たないため。詳細は
  [`docs/metrics_ja.md`](docs/metrics_ja.md) 参照)。1回の実行につき分位点は1つ - 2つ目の分位点
  が必要なら `compare` をもう一度実行してください。
* **`relative-diff`** - baseline/candidateのペア数値レコードに対する
  `(candidate - baseline) / baseline` のブートストラップ信頼区間: `mean-diff` の絶対変化ではなく、
  baselineに対する比例変化です。すべてのbaselineが厳密に正(`baseline > 0`)であることが必須です -
  ゼロや負のbaselineは設定/データのエラーとして拒否され、`abs(baseline)` や分母のオフセットで黙って
  計算されることはありません。`--bootstrap-method percentile`/`basic`/`bca`/`--resamples`/`--seed`
  は `mean-diff` と同じだけ対応しています。`effect`/`ci_low`/`ci_high` は比率(`0.05` = +5%)として
  報告され、Markdownではパーセンテージとして表示されます(`+5.0%`)- `winrate` のパーセンテージ
  ポイント(`pp`)接尾辞とは区別されます。今回未対応: `veridict power --metric relative-diff` と
  `--claim-correction`(詳細は[`docs/metrics_ja.md`](docs/metrics_ja.md)参照)。

「1単位の差」が全レコードで同じ実務的意味を持つ場合は `mean-diff` を使ってください。事前に定めた
評価対象が比例変化であり、baselineがすべて正である場合は `relative-diff` を使ってください。
この2つは同じ問いを異なる精度で答えるのではなく、異なる問い(平均絶対変化 vs. baselineに対する
平均比例変化)に答えるものです - 選ぶ前に知っておくべき点がいくつかあります:

* `relative-diff` は方向依存であり、両辺を入れ替えても対称にはなりません: +100%の増加の後に
  -50%の減少が続くと、元の値にちょうど戻ります。
* baselineがゼロまたは負の場合は使えません(`compare` は何を意図していたか推測せず、実行全体を
  拒否します)。
* 小さいbaselineは、小さな絶対的変化からでも大きな比を生み出し得ます - 単独の大きな
  `relative-diff` の外れ値を信頼する前に、データのbaselineスケールを把握してください。
* 各ペア自身の相対差の平均と、合計値どうしの比(`sum(candidate)/sum(baseline) - 1`)は、
  baselineのスケールがばらつく場合には一般に異なる値になります - `relative-diff` が報告するのは
  前者です(詳細は[`docs/metrics_ja.md`](docs/metrics_ja.md)参照)。
* 両方を見た*後で*たまたま通った方を選ぶのは、事前に定めた評価対象ではなく探索的な分析です -
  詳しくは下記の[スケール不整合の診断](#スケール不整合の診断)を参照してください。

`winrate` と `sign-test` は `effect`/`ci_low`/`ci_high` を0を中心とした値(五分五分からの偏差)として報告します。`elo` も構造上0を中心とします(五分の成績は0 Elo)。この3つはいずれも `--min-effect` とそのまま組み合わせられます。`mean-diff`/`quantile-diff` は入力そのものの単位で報告し、`relative-diff` は比率として報告します。

各トライアルの `baseline_status`/`candidate_status`(`timeout`、`crash`、`invalid`)は、どのメトリクスを実行してもタリーされ、レポートに含まれます。合計値だけでなく、どちら側で失敗したかの内訳(JSONレポートの `failure_breakdown`)も出力されます。`--failure-policy`(`compare --metric winrate`/`--metric elo` と `sprt` の全 `--sprt-variant` で使用可能)は、失敗がレポートだけでなく*計算そのもの*にも影響するかどうかを制御します:

* **`report-only`**(デフォルト) - このフラグが存在する前と変わりません: 失敗はタリーされますが、ステータスのみのレコード(`result` なし)はどちらにせよメトリクスに何も寄与しません。失敗ステータスと `result` の両方を持つレコードは、引き続き `result` がカウントされます。
* **`exclude`** - 失敗した側の `result` は、ステータスと同居していてもカウントされません。`report-only` と異なるのはこの混在ケースだけで、ステータスのみの一般的なケースはどちらでも同じ挙動です。
* **`loss`** - 失敗した側の結果を `result` から読む代わりに合成します: candidateが失敗 -> `baseline_win`、baselineが失敗 -> `candidate_win`、両方失敗 -> `draw`。これは同じレコード上の `result` を*上書き*します - `result` が何と言っていようと、失敗ステータスの方が信頼されます。

`exclude`/`loss` は勝敗ベースのメトリクス(`winrate`/`elo`)にのみ適用されます。`--metric mean-diff`/`--metric sign-test`/`--metric relative-diff` と組み合わせるのは設定エラーです - 失敗した数値トライアルに恣意的なペナルティを課す原理的な方法がないためです。

複数の `--metric` を同時に指定した場合も、入力全体のスキャンは1回だけです(メトリクスの数だけスキャンを繰り返すのではなく、1回のスキャンで全メトリクスに各レコードを渡します)。

## スケール不整合の診断

`--metric mean-diff` を実行すると、baselineがすべて正でありながら少なくとも約10倍の範囲にわたる
場合に `data_quality.wide_baseline_scale`(助言のみで `verdict` は変更しません)が発火します -
絶対差が最大規模のケースに支配されている可能性があるというサインです:

```console
$ veridict compare examples/mixed_scale_scores.jsonl --metric mean-diff --min-effect 0
...
"warnings": [
  "small sample: 15 paired trial(s), below the conventional 30-trial threshold for confidence-interval methods to be reliable",
  "baseline values span 1.8 orders of magnitude; absolute differences may be dominated by larger-scale cases. If the scientific question is proportional change, consider --metric relative-diff. Choose the metric before confirmatory analysis; switching after inspecting the verdict is exploratory."
]
```

baselineにゼロまたは負の値が含まれる場合、警告は代わりにドメインに即した正規化を提案します
(`relative-diff` はそのデータに対しても同様にwell-definedではないため):

```text
baseline values vary widely, but some baselines are zero or negative, so
relative-diff is not well-defined for this dataset. Use a domain-justified
normalization rather than adding an arbitrary denominator offset.
```

**この診断は、観測された結果に基づいてメトリクスの切り替えを推奨することは決してありません。**
baselineの値のみから計算されます - `positive_baseline_count`、`non_positive_baseline_count`、
`min_positive_baseline`、`max_positive_baseline`、`raw_orders_of_magnitude`、そして(外れ値1件が
支配しないよう、正のbaselineが20件以上のときのみ)`robust_orders_of_magnitude` が
`mean-diff`/`relative-diff` のレポート上で `scale_diagnostics` として公開されます - candidateの値、
効果量、CI、判定は一切見ません。確証的な分析として扱う前に、測定目標に基づいて
`--metric mean-diff` か `--metric relative-diff` かを選んでください。判定結果を見た後にメトリクスを
切り替えることは、2つ目の確証的な分析ではなく探索的なものであり、新規または保留にしていたデータで
検証すべきです。正確なしきい値と警告文のルールは [`docs/metrics_ja.md`](docs/metrics_ja.md) を
参照してください。

## 多重比較補正

複数の `--metric` を同時に実行するということは、*個々のメトリクス自身の* `verdict`/`promotion`
のどれか1つが偶然しきい値を超えてしまう独立したチャンスが複数あるということです。
`--claim-correction bonferroni`/`holm` は、それらを「同時に成立した複数の主張」として読んだときの
リスクを、補正なしの単一メトリクスが今日すでに持っているリスク以下に保ちますが、`compare` が出す
deployment-gateの `verdict`/`promotion` には一切触れません - 全metricのpassを要求する現在の
集約ルールは、それだけで既に単一メトリクス以上に保守的であり、その保証のために補正は不要だから
です(詳しい理由は [`docs/metrics_ja.md`](docs/metrics_ja.md)の `--claim-correction` セクションを
参照)。代わりに、補正はレポートごとの `family_adjusted_verdict`/`family_adjusted_promotion`
(併せて `unadjusted_verdict` も現れますが、これは常に `verdict` と同値を返す非推奨の互換エイリアス
です - 代わりに `verdict` を読んでください)と、実行全体の `simultaneous_claims_promotion` という
別のフィールドに反映されます。`--cluster-by-id`
や `--metric mean-diff`/`quantile-diff`/`relative-diff` との併用はサポートされておらず、設定エラー
として拒否されます(理由は `docs/metrics_ja.md` 参照)。`--correction` は1リリースだけ非推奨エイリアスとして
残ります。デフォルトは `none` - オプトインしない限り、今日と全く同じ挙動のままです。

```console
$ veridict compare examples/chess_engine_multi_metric.jsonl --metric winrate --metric elo --min-effect 0.02 --claim-correction bonferroni
{
  "schema_version": 1,
  "verdict": "pass",
  "promotion": "promoted",
  "simultaneous_claims_promotion": "not_promoted",
  "reports": [
    {
      "verdict": "pass",
      "promotion": "promoted",
      "metric": "winrate",
      "reason": "CI lower bound 0.0221 meets the pass threshold 0.0200",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.045327562117809694,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass",
      "family_adjusted_verdict": "inconclusive",
      "family_adjusted_promotion": "not_promoted"
      // ...
    },
    {
      "verdict": "pass",
      "promotion": "promoted",
      "metric": "elo",
      "reason": "CI lower bound 15.3650 meets the pass threshold 0.0200",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.016420872210740903,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass",
      "family_adjusted_verdict": "pass",
      "family_adjusted_promotion": "promoted"
      // ...
    }
  ]
}
```

`winrate` の証拠は本物ですが相対的に弱く、2つに分けるとその補正後のしきい値をもう超えられなく
なるため、*そのメトリクス自身の* `family_adjusted_verdict` が `inconclusive` に下がり、全体の
`simultaneous_claims_promotion` も `not_promoted` になります。deployment-gateの
`verdict`/`promotion` は最後まで `pass`/`promoted` のままです - 両方のメトリクスとも、補正前の
信頼水準では引き続き個別にpassしており、組み合わさった結果はその値から作られているからです。
`--claim-correction holm` は同じ保証のもとで `bonferroni` より一様に検出力が高く(この例では
両方のメトリクスのfamily-adjusted claimが `pass` のままになります)、どちらも補正前のpassを
inconclusiveに格下げすることしかできず、failを新たに作り出すことはありません。

## レポートの追加情報

すべてのレポート(`compare`、`sprt`、`matrix` いずれも)には `schema_version` という整数フィールドが
含まれます(現在は `1`)。純粋な追加変更(新しいフィールド、新しいenumバリアント)の間はこの値は
変わらず、フィールドの削除・改名があったときにのみ増分されます - そのため、機械側の消費者はフィールド
の有無から推測するのではなく、このバージョン番号でパース方法を切り替えられます。レポート/レコード
ごとのJSON Schemaは [`schemas/`](schemas/) を参照してください。

`compare` のレポートには、`verdict` に影響しない付加的なフィールドも常に含まれます:

* **`estimated_additional_trials`** - `inconclusive` な結果を決着させるのに必要な追加トライアル数のおおまかな見積もり(信頼区間が `O(1/√n)` で縮小するという前提、効果量自体は変わらないと仮定)。提案できることが何もない場合は `null` になります - 既に判定済み、トライアル数が0件、あるいは効果量がpass/failしきい値の"内側"(デッドゾーン)にある場合です: 効果量がすでにデッドゾーン内にある点推定を中心に信頼区間を縮めても、データをどれだけ追加してもどちらの境界も越えられません。この数値は「保証」ではなく「だいたいこのくらい、あるいはもっと必要」という目安として扱ってください - 既知の、定量化されたバイアスがあります(検証済みの一例では n=100 で約18%の過小評価)。
* **`warnings`** - 人間可読なデータ品質の警告で、何もなければ空です: サンプルが小さい(ペアトライアルが30件未満)、失敗率が高い(timeout/crash/invalidが20%超)、`elo` で引き分けが多い(引き分けが50%を超えると、レーティングの根拠となる決着済みの結果が少なくなります)、測定された効果量がCI自身の半値幅より小さい(ゼロ周りのノイズである可能性がある)、`quantile-diff` で要求された分位点の薄い方の裾の期待観測数が10件未満(`paired_count * min(q, 1-q)`)、`mean-diff` でbaselineが広い範囲にわたる(上記の[スケール不整合の診断](#スケール不整合の診断)参照)、またはunpairedモードで同一の`id`が10件以上のid付きトライアル中3回以上繰り返されている(すべての`id`がちょうど2回ずつ出現する場合 - つまり`--paired-by-id`を付け忘れただけのよくあるケース - は発火しません)場合です。
* **`data_quality`** - `warnings` と同じ内容を、文字列ではなく真偽値(`tiny_sample`、`high_failure_rate`、`draw_heavy`、`effect_within_noise_floor`、`low_id_diversity`、`thin_quantile_tail`、`wide_baseline_scale`)として持つフィールドです。文章を解析するのではなくフラグで分岐したい機械側の消費者向けです。`warnings` を置き換えるものではなく併存します - どちらも常に存在します。
* **`scale_diagnostics`** - `mean-diff`/`relative-diff` のみ: `wide_baseline_scale` の算出元となる
  baseline値の生の分布(`positive_baseline_count`、`non_positive_baseline_count`、
  `min_positive_baseline`、`max_positive_baseline`、`raw_orders_of_magnitude`、
  `robust_orders_of_magnitude`)です。詳細は[スケール不整合の診断](#スケール不整合の診断)参照。

各手法の前提・失敗モードの詳細は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

## SPRT

`veridict sprt` は `compare` とは別のモードです。効果量としきい値と照合する信頼区間の代わりに、対数尤度比を累積し、`--alpha`/`--beta` から導かれる2つの境界のいずれかを超えた時点で確定します。`wald`/`trinomial` の場合はこうなります: 増え続ける入力に対して `sprt` を再実行すると、そのたびに入力全体からLLRを再計算するので、呼び出し側が `pass`/`fail` を受け取った時点で止めれば、その*一連の呼び出し*全体として逐次検定の挙動になります。`--sprt-variant pentanomial` はさらに一歩進んでおり、呼び出し側にそうした規律を求めません: *1回*の呼び出しの中で、完了したペアを完了順に再生し、判定が下る最初のペアで内部的に停止します。そのため、その停止点より後まで続くファイルを渡しても結果は変わりません(詳細は後述)。`pass` は「候補が少なくとも `--elo1` ポイント強いと確信できる」、`fail` は「高々 `--elo0` ポイントの強さだと確信できる」、`inconclusive` は「データ収集を継続する」ことを意味します。`--alpha`/`--beta` はレポート上の調整可能なつまみではなく、実際に保証される偽陽性率/偽陰性率そのものです。このサブコマンドに `--min-effect`/`--confidence` はありません。3つのバリアントがあります(`--sprt-variant`):

* **`wald`**(デフォルト) - 決着がついた(引き分けを除く)`result` レコードのみに対する古典的な二値SPRT。このモデルの下では引き分けはどちらのElo仮説が正しいかについて情報を持たないため、LLRから完全に除外されます。仮説は `--elo0`/`--elo1` で、標準的なロジスティックEloです。
* **`trinomial`** - 引き分けを考慮した一般化LLR検定(チェスエンジンのテストツール、例えばFishtestで歴史的に使われてきたBayesEloパラメータ化)。引き分け率をプールされた勝ち/引き分け/負けのカウントからニュイサンスパラメータとして推定することで、引き分けの多いデータで `wald` より速く収束します。**単位はロジスティックEloではなくBayesEloです** - この2つは推定された引き分け率がちょうどゼロのときのみ一致するため、`--elo0`/`--elo1` を再解釈するのではなく、別途 `--belo0`/`--belo1` フラグで仮説を与えます。推定された引き分け率(`drawelo`)は、判定に使われているのと同じデータから推定されたものであるため、透明性のため出力に含まれます。
* **`pentanomial`** - ペア対局(同じ開始局面、先後入れ替え)に対する一般化LLR検定(Fishtestの `LLR_logistic`)。**常に `--paired-by-id` が必須です**: 同じ `id` を持つ2レコードを、ペアの合計スコア(候補側の得点をペアの2局で合計した `0`/`0.5`/`1`/`1.5`/`2`)による5値カテゴリに結合します - `winrate`/`elo` の `--paired-by-id` のように単一の勝敗/引き分けに正味化するのではありません。同じ `id` がちょうど2回出現しない場合は即座にエラーになります(ペアなしサンプルとして黙って扱うことはしません) - 5値のペアスコアは1局だけでは意味を持たないためです。仮説は `--elo0`/`--elo1` で、`wald` と同じロジスティックEloです - このモデルにはBayesEloを意味あるものにするような `drawelo` 相当のニュイサンスパラメータが存在しません。

  **なぜ「trinomialを2倍の局数で回す」のと同じではないのか:** pentanomialペアの統計的な価値は、その2局間の**負の相関**からもっぱら生まれます。同じ開始局面を先後入れ替えて指す設計では、局面の偏りが一方の対局では候補側に有利に、もう一方では不利に働くため、ペアの合計スコアの期待値はその偏りの大きさによらず一定になります(`+b` と `-b` が打ち消し合う)。つまりペア合計にはその偏りに由来する分散が乗らない一方、個々の対局を独立に見ると(偏りの分布で平均した)周辺分散は通常のサンプリング分散に加えてその偏り由来の分散で膨らみます。`trinomial`/`wald` をペア化前の個々の対局にそのまま適用すると、この膨らんだ分散をそのまま受け取ってしまいますが、`pentanomial` のペア単位の集計はそれを打ち消します - これが、実際のペア対局データにおいて `pentanomial` が `trinomial` よりも少ないペア数で収束しうる理由であり、単に「同じ情報をまとめて渡しているだけ」ではありません。

  レポートには `sprt_variant`(全バリアント共通)に加えて、`pentanomial` のときだけ `pentanomial_counts`(LLR計算に使った5値の内訳)、`raw_trial_count`(ペア化前の入力レコード総数)、`paired_count`(逐次停止点まで実際に解析されたペア数)、`available_paired_count`(入力に存在するペアの総数)、`stopping_pair_count`/`stopping_reason`(どこで・なぜ停止したか)、`ignored_pairs_after_stop`(入力には存在するが停止点より後に完了したため解析されなかったペア数)が追加されます。既存の `candidate_wins`/`baseline_wins`/`draws` も、同じ5値を通常の「ペア対局」規約(合計 `>1` は候補側の正味勝ち、`<1` は基準側の正味勝ち、ちょうど `1` は正味引き分け)で正味化した値として引き続き入ります。

3つのバリアントすべての詳細な仕組み(BayesEloとロジスティックEloの単位変換を含む)は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

`sprt` も `--failure-policy` を受け付けます(正確な `report-only`/`exclude`/`loss` の意味は
[メトリクス](#メトリクス)を参照) - 3つの `--sprt-variant` すべてで同じように適用され、
`pentanomial` でも `loss` によって合成された結果がペアの相手と通常どおり正味化されます。

`--sprt-variant pentanomial` は次のフラグも受け付けます:

* **`--min-paired-ids`**(`1` 以上が必須) - この数のペアが完了するまで、逐次処理は境界超過の
  判定そのものを一切行いません。そのため、少なすぎるデータで見えた早期の境界超過が、最小値に
  達した時点で「思い出されて」判定に使われることはありません - 実際に確定するのは、その時点まで
  に完了したすべてのペアを使って計算した、逐次処理が実際に停止したペアでのLLRです。最小値未満で
  は、蓄積されたLLRがどこにあっても `verdict` は `inconclusive` のままで、`validity` は変更され
  ません(これは「データがまだ足りない」であって「run が壊れていた」ではないためです)。
* **`--max-paired-ids`**(両方指定する場合は `--min-paired-ids` 以上が必須) - 境界超過なしに
  完了ペア数がこの値に達すると、そこで `inconclusive` 判定のまま停止します(打ち切りSPRTのような
  強制判定ルールはありません)。`reason`/`stopping_reason` に上限到達が記載されます。入力に存在
  していても、この時点より後に完了したペアは判定・LLR・バケット集計のいずれにも影響しません。
* **`--require-complete-pairs`** - `pentanomial` が既に無条件で行っている「すべてのidがちょうど
  2回出現する、例外なし」というペアリングを `wald`/`trinomial` にも適用します(`--paired-by-id`
  が必須。通常そこでは単独のidはペアなしサンプルとして許容されます)。`pentanomial` 自体に対して
  はこのフラグの有無によらず既にこの厳格さであるため、何もしないフラグとなります。

レポートにはこの3つがそれぞれ `min_paired_ids`、`max_paired_ids`、`require_complete_pairs`
として追加されます。

## 時間価値付き検定(time-sensitive testing)

`veridict sprt` が答えるのは「候補は決定的に優れているか?」ですが、`veridict time-sensitive` は
別の問いに答えます: 早期の棄却ほど価値が高くなる報酬(締切、あるいは時間とともに減衰する価値)を
与えたとき、対立仮説のもとで期待報酬を最大化する賭け方はどれか? - しかも `sprt` とまったく同じ
第一種過誤の保証を、どんな賭け方を選んでも維持したまま。これは独立した追加機能であり、既存機能の
置き換えではありません: この機能が存在することによって `sprt`/`compare`/`power` の挙動・JSON・
公開APIが変わることはありません。

> 本実装は、以下の数理的枠組みから独立に導出されたものです:
>
> E. Clerico, T. Wegel, I. Azangulov, and P. Rebeschini, "Time-sensitive anytime-valid testing,"
> arXiv:2605.06521v1, 2026. 論文は [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) の
> もとでライセンスされています。
>
> 本実装は改変された独立の実装であり、論文の著者はこのソフトウェアを公認・保守するものでは
> ありません。

**v1のスコープは意図的に狭く、Bernoulliのsimple-vs-simple(単純仮説同士)のみに限定しています** -
固定の帰無仮説 `p0` と対立仮説 `p1`(`0 < p0 < p1 < 1`)、一方向の棄却のみです。`p0`/`p1` は
*decisive-observation-conditional*(決着のついた観測に条件づけた)成功確率です - これは
`sprt --sprt-variant wald` が内部で `elo0`/`elo1` に使っているのと同じ規約です: `draw`(引き分け)
はtrial数と報酬スケジュールの時計を進めますが、どちらの仮説が正しいかについての情報は持たないため、
wealthプロセスを一切動かしません。trinomial/pentanomial(引き分けを考慮した)時間価値付きポリシー、
複合仮説、オンラインな `p1` 推定は今回のスコープ外です - [`docs/research-map.md`](docs/research-map.md)
を参照してください。

3つのポリシー(`--policy`):

* **`gro`**(growth-rate-optimal) - 古典的なanytime-validのベースライン: 報酬スケジュールを無視して
  毎回 `p1` に賭けます。`sprt` 自身のWald対数尤度比の歩みとbetting的に等価です(どちらも毎回、
  厳密な尤度比に賭けています)が、実装は独立しています(`sprt` のバッチ/集計カウント型APIには、
  時刻付きの棄却という概念がありません - 詳細は `veridict::time_sensitive` のモジュールdoc参照)。
  `gro` は `edo` の `time_scale` を大きくした極限に一致します。
* **`bellman`** - 任意の報酬スケジュールに対して、有限の `(action_grid_size, wealth_grid_size)`
  グリッド上で時間価値付き最適ポリシーを数値的に近似します。レポート上は
  `bellman_grid_approximation` と表記され、「最適」とは決して呼びません - あくまでグリッド上の
  数値近似であり、連続なaction/wealth空間での最適性の証明ではありません。
* **`edo`**(exponential-decay-optimal) - 閉じた形の*定常*(時間に依存しない)近似で、
  `--reward exponential` のときのみ有効です。グリッドを一切使わないため計算コストが低く、
  `--time-scale` が短いほど `gro` より積極的に賭けます。レポート上は `edo_stationary_approximation`
  と表記され、「Bellman最適」とは呼びません - 時刻`t`がwealthに依存する実効的な締切にどれだけ
  近いかを無視することが、まさに定常で安価であることの代償です。

**`bellman`/`edo` の近似が、なぜ保証を一切弱めないのか。** このモジュールのどのポリシーも、
最終的には各trialでaction `a`(`(0,1)` 区間)を選び、H0のもとで*どんな*`a`でも有効なBernoulli
e-variableをwealthに掛け合わせているだけです。したがってグリッド解像度・フロアの打ち切り・
閉じた形の近似誤差は、真の対立仮説のもとでwealthが*どれだけ速く*成長するか(最適性)にしか影響
せず、帰無仮説のもとで `1/alpha` を超える確率が `<= alpha` に収まるかどうか(validity)には
一切影響しません。この分離は、小さいホライズンについては `2^T` 通りの全経路列挙によって厳密に
(シミュレーションではなく)証明されており、現実的な規模(`T=400`)ではモンテカルロによる較正で
確認しています - 詳細は `time_sensitive` 自体のテストスイートを参照してください。

報酬スケジュール(`--reward`、独自スケジュールなら `--reward-schedule FILE`):

```console
$ veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
    --p0 0.50 --p1 0.55 --alpha 0.05 \
    --policy bellman --reward hard-deadline --deadline 400
{
  "verdict": "pass",
  "method": "bellman_grid_approximation",
  "reward_kind": "hard_deadline",
  "reward_parameters": { "deadline": 400 },
  "trial_count": 313,
  "rejection_time": 313,
  "reward_at_rejection": 1.0,
  "planned_expected_reward_under_p1": 0.645940711706933,
  ...
}
```

```console
$ veridict time-sensitive examples/chess_engine_time_sensitive.jsonl \
    --p0 0.50 --p1 0.55 --alpha 0.05 \
    --policy edo --reward exponential --time-scale 800 --horizon 3200
{
  "verdict": "pass",
  "method": "edo_stationary_approximation",
  "reward_kind": "exponential_decay",
  "reward_parameters": { "time_scale": 800.0, "horizon": 3200 },
  "trial_count": 218,
  "rejection_time": 218,
  ...
}
```

独自スケジュール(`examples/time_sensitive_reward_schedule.json`)はtrial数の区切りごとに報酬の
階層を宣言します。末尾(`after`)の値は必ず `0.0` にする必要があります - `t -> infinity` で
`R(t) -> 0` となることは、有限ホライズン問題として well-posed であるための条件そのものであり、
この形式で他の値を表現することはできません:

```json
{
  "rewards": [
    {"until": 400,  "value": 1.0},
    {"until": 1600, "value": 0.4},
    {"until": 3200, "value": 0.1},
    {"after": 3200, "value": 0.0}
  ]
}
```

**判定の意味論は一方向です。** `pass` はwealthが `1/alpha` を超えたことを意味し、`inconclusive`
は棄却前に報酬スケジュールのホライズンに達したことを意味します - このモデルには下側の棄却境界が
存在しないため、統計的な失敗としては決して解釈しません(棄却が起きなかったことはH0の証拠ではなく、
単にH1の証拠がまだ無いというだけです)。`promotion` は `validity: "valid"` かつ
`verdict: "pass"` のときだけ `promoted` になります - 他のすべてのサブコマンドと同じルールです。
`--failure-policy`/`--max-timeouts`/`--max-crashes`/`--max-invalid`([メトリクス](#メトリクス)参照)
は `sprt` と全く同じように動作し、上限超過時に `validity: "invalid"` と `verdict: "inconclusive"`
を強制します。

**終了コードは `0`(pass)/`2`(inconclusive)/`3`(設定/入力エラー)のみで、`1` は返しません。**
このモデルには両側の失敗という概念がないため、このサブコマンドが終了コード `1` を返すことは
ありません。

`planned_expected_reward_under_p1` には1つ注意点があります: これはシミュレーションではなく、
ポリシー自体が構築されたのと同じグリッドを使って*厳密に*計算されますが、全trialが決着すると
仮定した理想化のもとでの値です - 価値の漸化式にはdraw率の入力がないため、drawの多い入力
ストリームでは実際に得られる報酬はこの数値より低くなります(drawは報酬スケジュールの時間を
消費しますが、この漸化式はdrawを一切モデル化していません)。各レポートの `notes` に、上記の
スコープ・近似に関する注意点とあわせて明記されています。

## 比較マトリクス(comparison matrix)

`veridict matrix` は3つ以上の候補を比較し、ペアワイズのElo差を一覧表にします。レポート専用で(判定なし)、成功時は常に終了コード0です: マトリクス全体に対する単一のpass/failは存在しません。データの与え方は2通りあり、1回の実行で自由に組み合わせられます:

* **レガシー方式**: 候補ごとに1ファイル、いずれも*同じ共有ベースライン*に対して測定されたもので、`--metric elo`/`--metric winrate` と同じ `result` フィールドの勝敗/引き分けレコードを使います。
* **`--matches`**(繰り返し指定可): 名前付き対戦相手同士の直接対戦レコード - `{"id": ..., "a": "...", "b": "...", "result": "a_win"|"b_win"|"draw"}` - により、候補同士が共有ベースラインを介さず直接対戦したデータを扱えます。`a`/`b` に文字列 `"baseline"` を指定すると、レガシーファイルが暗黙に持つbaselineノードと接続できます。

得られたグラフがトポロジー的にまだスター形(どの対戦も出どころに関わらずbaselineを含む)であれば、`matrix` は閉形式を使います: 各候補のレーティングはそのままその候補自身のElo-vs-baselineです(スターグラフ上のBradley-TerryのMLEには共同で解くべき共有項が存在しません)。候補同士の実対戦データが存在する場合は、一般Bradley-Terryモデル(グラフ全体に対する反復ソルバー)を使ってフィットします。いずれの場合も、各セルには次のいずれかが付与されます:

* **`direct`** - その行と列の間に実際の直接対戦データが存在する。
* **`inferred`**(Markdown表では `*`)- 両者ともレーティングは付いており比較可能だが、直接対戦したことはない - モデルによる外挿 `elo_i - elo_j`。
* **`disconnected`**(Markdown表では `n/a`)- 両者を結ぶ経路が存在しない(例: 共通の対戦相手を持たない2つの独立した対戦クラスタ)。この場合、両者の間には不確実なレーティング差があるのではなく、有限のレーティング差そのものが存在しません - `elo_diff` は推測値ではなく `null` です。

スターグラフ/レガシー方式のセルは従来どおり実際のWilson区間を保持します。一般グラフモードのマトリクスセル(`direct`/`inferred`)も、実際のブートストラップ信頼区間を得られるようになりました: 各リサンプルではすべての対戦カードの勝敗/引き分けの実測比率からタリーを引き直し、グラフ全体を再フィットします。`ci_low`/`ci_high` は、そのペアが同じコンポーネントに留まったリサンプルにおける `elo_i - elo_j` から得られます。`matrix` の `--resamples`(デフォルト2,000)、`--seed`、`--bootstrap-method percentile`(デフォルト)/`basic`/`bca`(`compare` の同名フラグと同じ3手法・同じ意味)でこれを制御できます(いずれもスターグラフモードでは無視され、閉形式のWilson区間のままです)。`elo_diff` が付いているのに `ci_low`/`ci_high` が `null` のままのセルもあり得ます - これは実測データ上は繋がっているものの、リサンプリングに対してその接続が脆弱すぎる(リサンプルの90%未満でしか同じコンポーネントに留まらない)ため、誤って狭い区間を報告するよりは「信頼区間なし」と明示することを選んだ結果です。`CandidateSummary` 自体の `ci_low`/`ci_high` は、一般グラフモードでは引き続き常に `null` です: 個々のレーティングはそのコンポーネント内の任意の基準対戦相手との相対値でしかなく、`elo_i - elo_j` の信頼区間とは異なり、それ自体に信頼区間を付けると誤解を招くためです。

## Plan

`veridict plan` は `matrix` とまったく同じ入力(レガシーファイルと `--matches` を自由に組み合わせ可能)に加えて、必須の `--min-elo <f64>`(検出したいElo差)を受け取り、追加トライアルによって最も恩恵を受けるペアを不確実性の高い順に推薦します:

```console
$ veridict plan candidate_a.jsonl candidate_b.jsonl --min-elo 100
{
  "schema_version": 1,
  "min_elo": 100.0,
  "recommendations": [
    { "row": "baseline", "col": "candidate_b", "status": "direct",
      "current_ci_half_width": 254.6, "estimated_additional_trials": 53, "note": null },
    { "row": "candidate_a", "col": "candidate_b", "status": "inferred",
      "current_ci_half_width": 274.4, "estimated_additional_trials": 52, "note": null },
    { "row": "baseline", "col": "candidate_a", "status": "direct",
      "current_ci_half_width": 102.4, "estimated_additional_trials": 4, "note": null }
  ]
}
```

`matrix` と同様レポート専用です: 判定なし、成功時は常に終了コード0です。各推薦は `matrix` のセル1つに対応し、以下を含みます:

* **`current_ci_half_width`** - そのセルの現在のCI半値幅。narrowにする対象となるCIがまだ存在しない場合は `null`(`disconnected` なペア、またはリサンプリングに対して脆弱すぎて信頼できるCIが得られない `direct`/`inferred` セル - どちらも上の `matrix` の説明を参照)。
* **`estimated_additional_trials`** - 現在のCIがすでに `--min-elo` を満たしていれば `0`。`current_ci_half_width` が `null` になるのと同じセルでは、理由を説明する `note` とともに `null` になります。Disconnectedなペアはリストの先頭にソートされます(データが繋がるまで推定自体が不可能で、有限だが幅の広いCIよりも強い必要性があるため)。それ以外は推定値が大きい順にソートされます。

元の広いアイデアから削られたもの: `--budget N`/`--goal identify-best` という制約付き割り当てアルゴリズム。どちらも現時点でこのコードベースには実アルゴリズムが存在しません - 意図的に見送っているものの一覧は [`docs/research-map_ja.md`](docs/research-map_ja.md) を参照してください。

## Power

`veridict power` は、`compare --metric winrate/sign-test/elo` が合格判定に到達する目標確率(検出力)のために何トライアル必要かを - *実際に何も実行する前に* - 見積もります。入力ファイルはありません: フラグからの純粋な計算です。

```console
$ veridict power --metric elo --min-effect 20 --assume-effect 35 --target-power 0.80
{
  "schema_version": 1,
  "metric": "elo",
  "ci_method": "wilson",
  "min_effect": 20.0,
  "assume_effect": 35.0,
  "confidence": 0.95,
  "target_power": 0.8,
  "estimated_trials": 4281,
  "achieved_power": 0.8043871725361499,
  "method": "exact_binomial_search",
  "notes": [
    "Assumes the true effect is exactly assume_effect; a smaller real effect needs more trials than this number, not fewer - this is a design estimate for how much data to collect, not a guarantee about what a real run will show."
  ]
}
```

2つの効果量が**両方とも必須**で、`--assume-effect` は `--min-effect` を上回っていなければなりません:

* **`--min-effect`** - 合格ライン。`compare --min-effect`/`--pass-above` と全く同じ意味です。
* **`--assume-effect`** - 実際に検出力を計算する対象の真の効果。真の効果を合格ラインと等しく設定して検出力を評価すると、その境界そのものにおけるCIの被覆確率の裏返し(`≈ 1 - confidence`)しか得られません - 横ばいで、トライアルをどれだけ追加しても `--target-power` に近づいていきません。理由(ルールだけでなく)は [`docs/metrics_ja.md`](docs/metrics_ja.md) の `power` セクションを参照してください。

`estimated_trials` は*厳密な*探索(`sum Binomial_pmf(n, p1, k) * [CI_lower(k,n) >= p0]`、教科書的な近似ではありません)により、`compare` 自身が使うのと同じ実際の `wilson`/`exact`/`jeffreys` CI関数に対して求められます(`elo` は `wilson` のみを受け付けます。`compare --metric elo` と同じです)。`--paired-by-id` は受け付けられますが数値は変わりません - 理由はdocsセクションを参照してください。

`--metric mean-diff` はその探索の代わりに閉じた形の計算です - ブートストラップ信頼区間には
「仮想nでのCI幅」を求める閉形式の関数が存在しないため、想定される差分の標準偏差をあなたから
与える必要があります。直接指定する(`--assume-sd`)か、実際のパイロットデータから推定します
(`--pilot FILE`):

```console
$ veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --assume-sd 0.15
{
  "schema_version": 1,
  "metric": "mean-diff",
  "ci_method": "normal",
  "min_effect": 0.02,
  "assume_effect": 0.1,
  "confidence": 0.95,
  "target_power": 0.8,
  "estimated_trials": 28,
  "achieved_power": 0.805703217265413,
  "method": "normal_approximation_closed_form",
  "notes": [
    "This is a normal approximation of compare --metric mean-diff's real bootstrap decision rule, not an exact search against it: there is no real data pre-experiment to bootstrap, so a normal model of the paired differences is the standard assumption. For skewed real diffs the bootstrap CI and this estimate will diverge - treat this as a design estimate for how much data to collect, not a guarantee about what a real run will show.",
    "assume_sd is the standard deviation of the paired difference (candidate - baseline), not either arm's own standard deviation - using an arm's SD here would understate the true variance for anything but a perfectly correlated pair."
  ],
  "assume_sd": 0.15,
  "sd_source": "assume-sd"
}
```

同じ標準偏差を、推測ではなく実際のパイロットデータから推定することもできます:

```console
$ veridict power --metric mean-diff --min-effect 0.02 --assume-effect 0.10 --pilot examples/pilot_scores.jsonl
```

**`assume_sd` はペアの*差分*(`candidate - baseline`)の標準偏差であり、どちらか一方の腕自身の
標準偏差ではありません** - ペアデザインでよくある典型的なラベル付けミスです。これを間違えると
下流のすべての数値が静かに壊れます。計算式、`z_conf` がなぜ両側の信頼水準分位点でなければ
ならないか(`power` 自身の2つの効果値を要求する設計や `--claim-correction` の `alpha/2` という
ファミリー目標を形作ったのと同じ正確性のポイントです)、そして `--pilot` の小サンプルに関する
注意点は [`docs/metrics_ja.md`](docs/metrics_ja.md) の `power --metric mean-diff` セクションを
参照してください。

`--sprt` は構造的に異なる問いに切り替わります: Waldの SPRT はその構成上、`n` に関わらず既に
`alpha`/`beta` のエラー率を保証しているため、目標検出力を探索して求めるべきサンプルサイズという
ものが存在しません。代わりに、`sprt` 自身が受け取るのと同じ `--elo0`/`--elo1`/`--alpha`/`--beta`
が与えられたときの、各仮説の下での*期待*トライアル数(Waldの用語で「平均サンプル数」)を報告します:

```console
$ veridict power --sprt --elo0 0 --elo1 20
{
  "schema_version": 1,
  "elo0": 0.0,
  "elo1": 20.0,
  "alpha": 0.05,
  "beta": 0.05,
  "expected_trials_under_h0": 1601,
  "expected_trials_under_h1": 1603,
  "method": "wald_asn_approximation",
  "notes": [
    "expected_trials_under_h0/h1 are the two endpoint cases (the true strength sitting exactly at elo0 or elo1) - a real candidate whose true strength lies between elo0 and elo1, the common case since you're running SPRT precisely because that strength is unknown, needs substantially more trials than either endpoint: a Wald SPRT's expected sample size peaks near the midpoint between the two hypotheses, not at either one. Budget above these two numbers, not at them, when the candidate's true strength is genuinely uncertain.",
    "Wald's classical Average Sample Number approximation - ignores \"overshoot\" (the LLR's excess past a boundary at the moment of stopping), so a real run typically needs somewhat more trials than this number in practice.",
    "Counts decisive trials only (same as --sprt-variant wald itself) - a draw-heavy testcase needs more real games than this number, since draws don't move the LLR at all. Use --sprt-variant trinomial/pentanomial for draw-heavy testing."
  ]
}
```

**この2つの数値は楽観的な両端であり、最悪ケースではありません。** 期待サンプルサイズが最大になる
のは、候補の真の実力が `elo0` と `elo1` の*間*にある場合です - これは実測された効果です(同じ
elo0/elo1/alpha/betaでどちらの端点よりも約1.6倍、`tests/calibration/sprt_asn_calibration.rs`
参照)。下記のオーバーシュートに関する注意点のような小さな補正ではありません。候補の真の実力が
本当に不確かな場合は、これらの数値より上で予算を組んでください。

計算式・出典・*測定済みの*オーバーシュートバイアス(単なる引用ではなく)については
[`docs/metrics_ja.md`](docs/metrics_ja.md) の `power --sprt` セクションを参照してください。

## Verify run

`veridict compare`/`sprt` が判定するのは run の*統計*であり、その統計の計算元となった生の入力
自体が構造的に健全かどうかを確認する手段はありません。`veridict verify-run` はそのギャップを
埋めます: 宣言された `manifest.toml` と実際の `games.jsonl` を受け取り、同じファイルから計算
される判定を信頼する*前に*、ペアリング・順序・混入・不透明な識別子の一貫性をチェックします。

```console
$ veridict verify-run examples/manifest.toml examples/games_with_contamination.jsonl
{
  "schema_version": 1,
  "validity": "invalid",
  "reason": "3 structural violation(s) found; see `violations`.",
  "violations": [
    {
      "check": "experiment_contamination",
      "id": "stray-op9",
      "lines": [5],
      "field": "experiment_id",
      "detail": "record's experiment_id 'exp-2025-11-02-old-run' does not match manifest's 'exp-2026-07-26-001'"
    },
    ...
  ],
  ...
}
```

**自己整合性のみを見ます。** `dataset_sha256`/`binary_sha256`/`weight_sha256`/`config_sha256`/
`experiment_id`(および後述の共有envelopeの残りのフィールド)を含むすべてのハッシュ・識別子
フィールドは、呼び出し側が与える不透明な文字列です: `verify-run` は `manifest.toml` が宣言する
値と、`games.jsonl` の各レコードが繰り返す値を比較し、ドリフトがないかを見るだけです。実際の
バイナリ・重み・コーパス・設定ファイルを開いたりハッシュを計算したり解釈したりすることは一切
ありません - それは呼び出し側の責任のままです。これはこのプロジェクトのドメイン非依存な
「結果を判定するが、結果を生成しない」という設計方針([`docs/research-map_ja.md`](docs/research-map_ja.md)
参照)に一致します。

6つのチェックが入力全体に対して無条件に実行され、最初の1件で止まらずに*すべて*の不整合を
収集します:

* **`pair_completeness`** - `games.jsonl` に現れるすべての `id` は、ちょうど2レコードに出現
  しなければなりません。
* **`global_index_uniqueness`** - 呼び出し側が宣言する `global_index` フィールドは重複して
  はいけません。
* **`role_consistency`** - 同じ `id` を共有する2レコードは、異なる2つの `role` 値を宣言しな
  ければなりません(「ペア内での色/先後反転」のドメイン非依存な代替表現です - `role` が何を
  意味するかは呼び出し側の規約次第であり、veridict自身は関知しません)。また、run全体を通じて
  使われる `role` のペアリングの組み合わせは1種類でなければなりません。
* **`schedule_order`** - ゲート対象(`include_in_gate != false`)のペアidの実際の初出順序は、
  `manifest.toml` が宣言する `schedule` と一致しなければなりません。
* **`experiment_contamination`** - レコードの `experiment_id`(存在する場合)はmanifestの
  ものと一致しなければなりません - 別のrunのゲームが混入していないかを検出します。
* **`environment_consistency`** - `dataset_sha256`/`binary_sha256`/`weight_sha256`/
  `config_sha256` を、それぞれ独立に検証します: レコードの値(存在する場合)はmanifestが
  宣言する値と一致しなければなりません - バイナリ・重み・データセットの入れ替わりや、
  run途中の設定変更を検出します。

裏付けとなるフィールドがmanifest/レコードのどこにも一度も現れないチェックは、正直に
`checks_skipped`(`warnings` にも反映)として報告されます - 黙って通過させる(誤った安心感を
与える)ことも、ハードエラーにする(すべてのオプションフィールドが揃うまでコマンドが使えなく
なる)こともありません。`baseline_status`/`candidate_status`(他のすべてのサブコマンドと同じ
`ok`/`timeout`/`crash`/`invalid` の語彙)は、可視化のために `failure_breakdown`/`timeouts`/
`crashes`/`invalid` に集計されるだけで、上限との照合はされません - それは `compare`/`sprt` の
`--max-timeouts`/`--max-crashes`/`--max-invalid` の役割です。

`report.paired_count` は*正式な、ゲート対象の*ペア数です: ちょうど2レコードの `id`-グループの
うち、burn-in ペア(いずれかのレコードが `include_in_gate: false`)を除いたものです - burn-in
ペアは `pair_completeness`/`role_consistency` では他のペアと同様にチェックされますが、この数値
には数えられず、ゲート対象runのペアリング規約にも投票しません。

`violations` は決定的な順序でソートされたのち500件で打ち切られます - 深刻に破損したrunが入力
全体に比例した量のレポートを生成しないようにするためです。`violation_count` は常に正確な合計を
保持し、上の配列がサンプルにすぎない(全件ではない)場合は `violations_truncated` が `true` に
なります。

**終了コードは通常の判定ベースのものではなく、`verify-run` 独自のものです**: 不整合が見つから
なければ `0`、1件以上見つかれば `1`(レポートの `validity` は `"invalid"` - 構造的な不変条件は
成立するかしないかのどちらかで「inconclusive」は存在しないため、`verdict` の3値の形は
ここには当てはまりません)、report自体が生成できない本当のパース/設定エラー(不正な
`manifest.toml`、未対応の `manifest_schema_version`、不正な `games.jsonl`、空の入力、または
上記3つのmanifest依存チェックが検証すべき対象を何も宣言していないmanifest)の場合にのみ `3`
です。

**共有experiment envelope。** `experiment_id`/`candidate_id`/`baseline_id`/`lineage_id`/
`dataset_sha256`/`split_sha256`/`teacher_manifest_sha256`/`binary_sha256`/`weight_sha256`/
`init_seed`/`split_seed`/`shuffle_seed`/`schema_version`/`validity` は複数リポジトリにまたがる
共有契約を構成します([`schemas/experiment-envelope.schema.json`](schemas/experiment-envelope.schema.json)
参照)。veridictを取り巻くツール群のどれもがこのファイルをそのまま(verbatim)vendorすることを
想定しており、あるツールが別のツールの識別子の意味を理解する必要なく、下流のツールが判定結果を
自身のプロビナンスストアと突き合わせられるようにします。`manifest.toml` の全体像(envelopeの
フィールドに加えて、共有envelopeには含まれない `config_sha256`/`schedule` を持つ)は
[`schemas/manifest.schema.json`](schemas/manifest.schema.json)、`verify-run` 用の
`games.jsonl` のレコード単位のスキーマは
[`schemas/verify-run-record.schema.json`](schemas/verify-run-record.schema.json) です
([`schemas/input-record.schema.json`](schemas/input-record.schema.json) とは別のスキーマです -
`verify-run` は指標を一切計算しないため `baseline`/`candidate`/`result` フィールドは不要で、
他のサブコマンドには存在しない複数のフィールドが必要になります)。

このコードベースの他の箇所と同じ注意点があります: 引用符付きフィールドに改行が含まれる場合、
CSVの行番号は物理的なファイル行ではなくレコードのインデックスになります - 「どのレコードが
悪いのかを指し示す」ことがこのコマンドの提供価値そのものなので、`games.jsonl` にはJSONLを
使うことを推奨します。

## ペアテストケース(paired testcases)

`--paired-by-id`(`compare`、`sprt`、`matrix` で使用可能)は、同じ `id` を持つ2つのレコードを「同じテストケースを2回実行したもの」(例: そのテストケース固有のバイアスを打ち消すために役割を入れ替えて再実行したもの)とみなし、2つの独立した観測ではなく1つの正味の観測として結合します:

* `winrate`/`elo`: ペア全体の合計ポイント(勝ち=1、引き分け=0.5、負け=0という、いわゆる「ペアゲーム」の標準的な採点方式)で正味化します - 合計が`1`より大きければ正味candidate勝ち、`1`未満なら正味baseline勝ち、ちょうど`1`なら正味引き分けです。
* `mean-diff`/`quantile-diff`/`sign-test`: ペアの2つの差分の平均で正味化します。
* `relative-diff`: 各生レコードをまず相対変換し(`(candidate - baseline) / baseline`)、その後
  ペアの2つの比を平均して正味化します - 先にbaseline/candidateを平均してから1つの比を取るのでは
  ありません(詳細は[`docs/metrics_ja.md`](docs/metrics_ja.md)参照)。

`id` が1回しか出現しない場合は通常のペアなしサンプルとして扱われます(1つのファイルにペアありとペアなしのテストケースが混在しても問題ありません)。同じ `id` を持つレコードが3つ以上ある場合は、ペアへ黙って切り詰めるのではなく、データエラーとして拒否されます。`--paired-by-id` を指定しない場合、`mean-diff`/`quantile-diff`/`sign-test`/`relative-diff` レコードの `id` 重複はこのフラグの有無に関わらず従来どおり拒否されます。

**`sprt --sprt-variant pentanomial` だけは「`id` が1回だけならペアなしサンプルとして扱う」という原則の例外です**: ペアを正味化せず5値のスコアをそのまま使うため([SPRT](#sprt)参照)、1局だけでは意味を持ちません。そのため常に `--paired-by-id` が必須で、ちょうど2回出現しない `id` は即座にエラーになります - 他のすべての箇所とは異なり、ペアなしサンプルとしては扱われません。

## 判定ロジック

このゲートは点推定ではなく信頼区間としきい値を比較します: `pass` は信頼区間の悲観的な(下側の)境界が `--pass-above` を上回ることを要求し、`fail` は信頼区間の楽観的な(上側の)境界が `--fail-below` 以下であることを要求します。それ以外(使用可能なトライアルが0件の場合を含む)はすべて `inconclusive` です。

`--min-effect X` は対称なしきい値(`--pass-above X --fail-below -X`)の省略形で、デフォルトは `0` です。

## 統計的根拠

veridict が出す数値は独自の謎スコアではなく、標準的な(査読済みの)統計手法に基づいています。
各メトリクスの前提・失敗モードまで含めた完全版は [`docs/metrics_ja.md`](docs/metrics_ja.md) を、
検討したが未実装の手法・意図的にスコープ外としているものは
[`docs/research-map_ja.md`](docs/research-map_ja.md) を参照してください。

* **`winrate`/`sign-test` の信頼区間** - Wilson score interval(Wilson 1927)。`--ci-method exact`
  を指定すると、代わりにClopper-Pearsonの正確な二項信頼区間(Clopper & Pearson 1934)になります。
* **`mean-diff`/`quantile-diff`/`relative-diff` の信頼区間** - percentile / BCa(バイアス補正・加速)
  ブートストラップ。いずれもEfron & Tibshirani『An Introduction to the Bootstrap』(1993年、14章)に
  基づきます。BCaがCLIレベルでゲートされているのは `quantile-diff` のみです(詳細は
  [`docs/metrics_ja.md`](docs/metrics_ja.md) 参照)- `relative-diff` は `mean-diff` と同じく3つの
  `--bootstrap-method` すべてに対応しています。
* **`elo`** - ロジスティックElo モデル。Eloの原型のレーティングシステム(Elo 1978)を、広く使われて
  いる形に変形したものです。
* **`sprt`** - Waldの逐次確率比検定(Wald 1945、`--sprt-variant wald`)。`trinomial`/`pentanomial`
  バリアントは、チェスエンジンのテストツール(Fishtestの `LLRlegacy`/`LLR_logistic`)で歴史的に
  使われてきたスタイルの一般化LLR検定です。
* **`matrix` の一般グラフモード** - Bradley-Terryのペア比較モデル(Bradley & Terry 1952)を、
  Zermelo(1929)/Hunter(2004)のMM(Minorization-Maximization)不動点法でフィットします。有限な解が
  存在するための条件はFord(1957)によります。

一方で、次の値は学術論文由来の厳密な統計的結果ではありません - 本プロジェクト独自の設計判断・経験則
であり、それを定理であるかのように装ってはいません:

* **`pass`/`fail`/`inconclusive`** - 信頼区間を閾値と比較すること自体は標準的な決定則ですが、どの
  閾値を使うか、および「false passはinconclusiveより悪い」という保守的な方針(判定ロジック参照)は、
  本プロジェクト独自の設計判断です。
* **`estimated_additional_trials`** - `winrate`/`sign-test`/`elo` では、レポートが実際に使っている
  CI計算式に対する二分探索であり、想定モデル(点推定を固定)のもとでは厳密です。例外は
  `mean-diff`/`quantile-diff`/`relative-diff`で、いずれのブートストラップCIにもそのような閉形式が
  存在しないため、3つとも `O(1/sqrt(n))` のスケーリングによる近似にフォールバックします。これには
  既知のバイアスがあります(レポートの追加情報を参照)。
* **`warnings`** - サンプル数30件・失敗率20%・引き分け率50%・(`quantile-diff`のみ)裾の期待観測数
  10件・(`mean-diff`のみ)baselineスケール約10倍といった閾値は、特定の論文由来ではなく慣習的な
  経験則です。

### 参考文献

- Wilson, E. B. (1927). "Probable Inference, the Law of Succession, and Statistical Inference."
  *Journal of the American Statistical Association*, 22(158), 209-212.
- Clopper, C. J.; Pearson, E. S. (1934). "The use of confidence or fiducial limits illustrated in
  the case of the binomial." *Biometrika*, 26(4), 404-413.
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*. Chapman & Hall/CRC.
- Wald, A. (1945). "Sequential Tests of Statistical Hypotheses." *Annals of Mathematical
  Statistics*, 16(2), 117-186.
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*. Arco Publishing.
- Bradley, R. A.; Terry, M. E. (1952). "Rank Analysis of Incomplete Block Designs: I. The Method
  of Paired Comparisons." *Biometrika*, 39(3/4), 324-345.
- Zermelo, E. (1929). "Die Berechnung der Turnier-Ergebnisse als ein Maximumproblem der
  Wahrscheinlichkeitsrechnung." *Mathematische Zeitschrift*, 29, 436-460.
- Hunter, D. R. (2004). "MM algorithms for generalized Bradley-Terry models." *Annals of
  Statistics*, 32(1), 384-406.
- Ford, L. R. Jr. (1957). "Solution of a Ranking Problem from Binary Comparisons." *The American
  Mathematical Monthly*, 64(8), 28-33.

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
