# veridict

[English](README.md) | 日本語

[![CI](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml/badge.svg)](https://github.com/kent-tokyo/veridict/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/veridict.svg)](https://crates.io/crates/veridict)
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
* **CIでのリリースリグレッションゲート** - 候補ビルドを直近の正常なベースラインと比較し、
  `--fail-below`/`--pass-above` と `veridict` の終了コードでパイプラインに組み込む
  (下記の[使い方](#使い方)のリグレッションゲート例を参照)。
* **3つ以上の案を比較** - 同じ共有ベースラインに対する複数のプロンプト/設定 →
  `veridict matrix`。

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

そのメトリクスの組に対して、偶然のpassを防ぐ([多重比較補正](#多重比較補正)を参照):

```bash
veridict compare results.jsonl --metric winrate --metric elo --min-effect 0.02 --correction holm
```

小さいサンプルには正確二項信頼区間、歪んだ分布にはBCaブートストラップ:

```bash
veridict compare results.jsonl --metric winrate --ci-method exact
veridict compare scores.jsonl --metric mean-diff --bootstrap-method bca
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

`winrate` と `sign-test` は `effect`/`ci_low`/`ci_high` を0を中心とした値(五分五分からの偏差)として報告します。`elo` も構造上0を中心とします(五分の成績は0 Elo)。この3つはいずれも `--min-effect` とそのまま組み合わせられます。`mean-diff`/`quantile-diff` は入力そのものの単位で報告します。

各トライアルの `baseline_status`/`candidate_status`(`timeout`、`crash`、`invalid`)は、どのメトリクスを実行してもタリーされ、レポートに含まれます。合計値だけでなく、どちら側で失敗したかの内訳(JSONレポートの `failure_breakdown`)も出力されます。`--failure-policy`(`compare --metric winrate`/`--metric elo` と `sprt` の全 `--sprt-variant` で使用可能)は、失敗がレポートだけでなく*計算そのもの*にも影響するかどうかを制御します:

* **`report-only`**(デフォルト) - このフラグが存在する前と変わりません: 失敗はタリーされますが、ステータスのみのレコード(`result` なし)はどちらにせよメトリクスに何も寄与しません。失敗ステータスと `result` の両方を持つレコードは、引き続き `result` がカウントされます。
* **`exclude`** - 失敗した側の `result` は、ステータスと同居していてもカウントされません。`report-only` と異なるのはこの混在ケースだけで、ステータスのみの一般的なケースはどちらでも同じ挙動です。
* **`loss`** - 失敗した側の結果を `result` から読む代わりに合成します: candidateが失敗 -> `baseline_win`、baselineが失敗 -> `candidate_win`、両方失敗 -> `draw`。これは同じレコード上の `result` を*上書き*します - `result` が何と言っていようと、失敗ステータスの方が信頼されます。

`exclude`/`loss` は勝敗ベースのメトリクス(`winrate`/`elo`)にのみ適用されます。`--metric mean-diff`/`--metric sign-test` と組み合わせるのは設定エラーです - 失敗した数値トライアルに恣意的なペナルティを課す原理的な方法がないためです。

複数の `--metric` を同時に指定した場合も、入力全体のスキャンは1回だけです(メトリクスの数だけスキャンを繰り返すのではなく、1回のスキャンで全メトリクスに各レコードを渡します)。

## 多重比較補正

複数の `--metric` を同時に実行するということは、*何か1つ*が偶然しきい値を超えてしまう独立した
チャンスが複数あるということです - `--correction bonferroni`/`holm` は、その組み合わさったリスク
を、補正なしの単一メトリクスが今日すでに持っているリスク以下に保ちます(詳しい理由は
[`docs/metrics_ja.md`](docs/metrics_ja.md)の `--correction` セクションを参照)。デフォルトは
`none` - オプトインしない限り、今日と全く同じ挙動のままです。

```console
$ veridict compare examples/chess_engine_multi_metric.jsonl --metric winrate --metric elo --min-effect 0.02 --correction bonferroni
{
  "schema_version": 1,
  "verdict": "inconclusive",
  "reports": [
    {
      "verdict": "inconclusive",
      "metric": "winrate",
      "reason": "CI lower bound 0.0221 meets the pass threshold 0.0200. Bonferroni correction (family_size=2): achieved significance 0.045328 exceeds the corrected threshold 0.025000 - downgraded from pass to inconclusive.",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.045327562117809694,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass"
      // ...
    },
    {
      "verdict": "pass",
      "metric": "elo",
      "reason": "CI lower bound 15.3650 meets the pass threshold 0.0200. Bonferroni correction (family_size=2) confirms: achieved significance 0.016421 <= the corrected threshold 0.025000.",
      "correction_method": "bonferroni",
      "family_size": 2,
      "achieved_alpha": 0.016420872210740903,
      "adjusted_alpha_threshold": 0.025000000000000022,
      "unadjusted_verdict": "pass"
      // ...
    }
  ]
}
```

`--correction` なしでは、この同じ入力に対して両方のメトリクスがpassし、全体の判定も `pass` に
なります。`winrate` の証拠は本物ですが相対的に弱く、2つに分けるとその補正後のしきい値をもう超え
られなくなるため、全体の判定は `inconclusive` に下がります - これはまさに本プロジェクトの土台と
なっている「false passはinconclusiveより悪い」という方針を、単一メトリクスの中だけでなくメトリクス
の組全体に適用したものです。`--correction holm` は同じ保証のもとで `bonferroni` より一様に検出力
が高く(この例では両方のメトリクスがpassしたままになります)、どちらも補正前のpassを
inconclusiveに格下げすることしかできず、failを新たに作り出すことはありません。

## レポートの追加情報

すべてのレポート(`compare`、`sprt`、`matrix` いずれも)には `schema_version` という整数フィールドが
含まれます(現在は `1`)。純粋な追加変更(新しいフィールド、新しいenumバリアント)の間はこの値は
変わらず、フィールドの削除・改名があったときにのみ増分されます - そのため、機械側の消費者はフィールド
の有無から推測するのではなく、このバージョン番号でパース方法を切り替えられます。レポート/レコード
ごとのJSON Schemaは [`schemas/`](schemas/) を参照してください。

`compare` のレポートには、`verdict` に影響しない付加的なフィールドも常に含まれます:

* **`estimated_additional_trials`** - `inconclusive` な結果を決着させるのに必要な追加トライアル数のおおまかな見積もり(信頼区間が `O(1/√n)` で縮小するという前提、効果量自体は変わらないと仮定)。提案できることが何もない場合は `null` になります - 既に判定済み、トライアル数が0件、あるいは効果量がpass/failしきい値の"内側"(デッドゾーン)にある場合です: 効果量がすでにデッドゾーン内にある点推定を中心に信頼区間を縮めても、データをどれだけ追加してもどちらの境界も越えられません。この数値は「保証」ではなく「だいたいこのくらい、あるいはもっと必要」という目安として扱ってください - 既知の、定量化されたバイアスがあります(検証済みの一例では n=100 で約18%の過小評価)。
* **`warnings`** - 人間可読なデータ品質の警告で、何もなければ空です: サンプルが小さい(ペアトライアルが30件未満)、失敗率が高い(timeout/crash/invalidが20%超)、`elo` で引き分けが多い(引き分けが50%を超えると、レーティングの根拠となる決着済みの結果が少なくなります)、測定された効果量がCI自身の半値幅より小さい(ゼロ周りのノイズである可能性がある)、`quantile-diff` で要求された分位点の薄い方の裾の期待観測数が10件未満(`paired_count * min(q, 1-q)`)、またはunpairedモードで同一の`id`が10件以上のid付きトライアル中3回以上繰り返されている(すべての`id`がちょうど2回ずつ出現する場合 - つまり`--paired-by-id`を付け忘れただけのよくあるケース - は発火しません)場合です。
* **`data_quality`** - `warnings` と同じ内容を、文字列ではなく真偽値(`tiny_sample`、`high_failure_rate`、`draw_heavy`、`effect_within_noise_floor`、`low_id_diversity`、`thin_quantile_tail`)として持つフィールドです。文章を解析するのではなくフラグで分岐したい機械側の消費者向けです。`warnings` を置き換えるものではなく併存します - どちらも常に存在します。

各手法の前提・失敗モードの詳細は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

## SPRT

`veridict sprt` は `compare` とは別のモードです。効果量としきい値と照合する信頼区間の代わりに、対数尤度比を累積し、`--alpha`/`--beta` から導かれる2つの境界のいずれかを超えた時点で停止します。`pass` は「候補が少なくとも `--elo1` ポイント強いと確信できる」、`fail` は「高々 `--elo0` ポイントの強さだと確信できる」、`inconclusive` は「データ収集を継続する」ことを意味します。`--alpha`/`--beta` はレポート上の調整可能なつまみではなく、実際に保証される偽陽性率/偽陰性率そのものです。このサブコマンドに `--min-effect`/`--confidence` はありません。3つのバリアントがあります(`--sprt-variant`):

* **`wald`**(デフォルト) - 決着がついた(引き分けを除く)`result` レコードのみに対する古典的な二値SPRT。このモデルの下では引き分けはどちらのElo仮説が正しいかについて情報を持たないため、LLRから完全に除外されます。仮説は `--elo0`/`--elo1` で、標準的なロジスティックEloです。
* **`trinomial`** - 引き分けを考慮した一般化LLR検定(チェスエンジンのテストツール、例えばFishtestで歴史的に使われてきたBayesEloパラメータ化)。引き分け率をプールされた勝ち/引き分け/負けのカウントからニュイサンスパラメータとして推定することで、引き分けの多いデータで `wald` より速く収束します。**単位はロジスティックEloではなくBayesEloです** - この2つは推定された引き分け率がちょうどゼロのときのみ一致するため、`--elo0`/`--elo1` を再解釈するのではなく、別途 `--belo0`/`--belo1` フラグで仮説を与えます。推定された引き分け率(`drawelo`)は、判定に使われているのと同じデータから推定されたものであるため、透明性のため出力に含まれます。
* **`pentanomial`** - ペア対局(同じ開始局面、先後入れ替え)に対する一般化LLR検定(Fishtestの `LLR_logistic`)。**常に `--paired-by-id` が必須です**: 同じ `id` を持つ2レコードを、ペアの合計スコア(候補側の得点をペアの2局で合計した `0`/`0.5`/`1`/`1.5`/`2`)による5値カテゴリに結合します - `winrate`/`elo` の `--paired-by-id` のように単一の勝敗/引き分けに正味化するのではありません。同じ `id` がちょうど2回出現しない場合は即座にエラーになります(ペアなしサンプルとして黙って扱うことはしません) - 5値のペアスコアは1局だけでは意味を持たないためです。仮説は `--elo0`/`--elo1` で、`wald` と同じロジスティックEloです - このモデルにはBayesEloを意味あるものにするような `drawelo` 相当のニュイサンスパラメータが存在しません。

  **なぜ「trinomialを2倍の局数で回す」のと同じではないのか:** pentanomialペアの統計的な価値は、その2局間の**負の相関**からもっぱら生まれます。同じ開始局面を先後入れ替えて指す設計では、局面の偏りが一方の対局では候補側に有利に、もう一方では不利に働くため、ペアの合計スコアの期待値はその偏りの大きさによらず一定になります(`+b` と `-b` が打ち消し合う)。つまりペア合計にはその偏りに由来する分散が乗らない一方、個々の対局を独立に見ると(偏りの分布で平均した)周辺分散は通常のサンプリング分散に加えてその偏り由来の分散で膨らみます。`trinomial`/`wald` をペア化前の個々の対局にそのまま適用すると、この膨らんだ分散をそのまま受け取ってしまいますが、`pentanomial` のペア単位の集計はそれを打ち消します - これが、実際のペア対局データにおいて `pentanomial` が `trinomial` よりも少ないペア数で収束しうる理由であり、単に「同じ情報をまとめて渡しているだけ」ではありません。

  レポートには `sprt_variant`(全バリアント共通)に加えて、`pentanomial` のときだけ `pentanomial_counts`(LLR計算に使った5値の内訳)、`raw_trial_count`(ペア化前の入力レコード総数)、`paired_count`(完全なペアの数)が追加されます。既存の `candidate_wins`/`baseline_wins`/`draws` も、同じ5値を通常の「ペア対局」規約(合計 `>1` は候補側の正味勝ち、`<1` は基準側の正味勝ち、ちょうど `1` は正味引き分け)で正味化した値として引き続き入ります。

3つのバリアントすべての詳細な仕組み(BayesEloとロジスティックEloの単位変換を含む)は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

`sprt` も `--failure-policy` を受け付けます(正確な `report-only`/`exclude`/`loss` の意味は
[メトリクス](#メトリクス)を参照) - 3つの `--sprt-variant` すべてで同じように適用され、
`pentanomial` でも `loss` によって合成された結果がペアの相手と通常どおり正味化されます。

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
ならないか(`power` 自身の2つの効果値を要求する設計や `--correction` の `alpha/2` という
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

## ペアテストケース(paired testcases)

`--paired-by-id`(`compare`、`sprt`、`matrix` で使用可能)は、同じ `id` を持つ2つのレコードを「同じテストケースを2回実行したもの」(例: そのテストケース固有のバイアスを打ち消すために役割を入れ替えて再実行したもの)とみなし、2つの独立した観測ではなく1つの正味の観測として結合します:

* `winrate`/`elo`: ペア全体の合計ポイント(勝ち=1、引き分け=0.5、負け=0という、いわゆる「ペアゲーム」の標準的な採点方式)で正味化します - 合計が`1`より大きければ正味candidate勝ち、`1`未満なら正味baseline勝ち、ちょうど`1`なら正味引き分けです。
* `mean-diff`/`quantile-diff`/`sign-test`: ペアの2つの差分の平均で正味化します。

`id` が1回しか出現しない場合は通常のペアなしサンプルとして扱われます(1つのファイルにペアありとペアなしのテストケースが混在しても問題ありません)。同じ `id` を持つレコードが3つ以上ある場合は、ペアへ黙って切り詰めるのではなく、データエラーとして拒否されます。`--paired-by-id` を指定しない場合、`mean-diff`/`quantile-diff`/`sign-test` レコードの `id` 重複はこのフラグの有無に関わらず従来どおり拒否されます。

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
* **`mean-diff`/`quantile-diff` の信頼区間** - percentile / BCa(バイアス補正・加速)ブートストラップ。
  いずれもEfron & Tibshirani『An Introduction to the Bootstrap』(1993年、14章)に基づきます。
  `quantile-diff` ではBCaはCLIレベルでゲートされています(詳細は
  [`docs/metrics_ja.md`](docs/metrics_ja.md) 参照)。
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
  `mean-diff`/`quantile-diff`で、どちらのブートストラップCIにもそのような閉形式が存在しないため、
  `O(1/sqrt(n))` のスケーリングによる近似にフォールバックします。これには既知のバイアスがあります
  (レポートの追加情報を参照)。
* **`warnings`** - サンプル数30件・失敗率20%・引き分け率50%・(`quantile-diff`のみ)裾の期待観測数
  10件といった閾値は、特定の論文由来ではなく慣習的な経験則です。

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
