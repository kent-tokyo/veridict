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

小さいサンプルには正確二項信頼区間、歪んだ分布にはBCaブートストラップ:

```bash
veridict compare results.jsonl --metric winrate --ci-method exact
veridict compare scores.jsonl --metric mean-diff --bootstrap-method bca
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

同じ共有ベースラインに対して測定した3つ以上の候補を一度に比較し、ペアワイズのElo差を一覧表にします:

```bash
veridict matrix prompt_a.jsonl prompt_b.jsonl prompt_c.jsonl
```

または、共有ベースラインなしで、直接対戦データから名前付き対戦相手をランク付けします:

```bash
veridict matrix --matches examples/matches_head_to_head.jsonl
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

`winrate` と `sign-test` は `effect`/`ci_low`/`ci_high` を0を中心とした値(五分五分からの偏差)として報告します。`elo` も構造上0を中心とします(五分の成績は0 Elo)。この3つはいずれも `--min-effect` とそのまま組み合わせられます。`mean-diff` は入力そのものの単位で報告します。

各トライアルの `baseline_status`/`candidate_status`(`timeout`、`crash`、`invalid`)は、どのメトリクスを実行してもタリーされ、レポートに含まれます。合計値だけでなく、どちら側で失敗したかの内訳(JSONレポートの `failure_breakdown`)も出力されます。

複数の `--metric` を同時に指定した場合も、入力全体のスキャンは1回だけです(メトリクスの数だけスキャンを繰り返すのではなく、1回のスキャンで全メトリクスに各レコードを渡します)。

## レポートの追加情報

すべてのレポート(`compare`、`sprt`、`matrix` いずれも)には `schema_version` という整数フィールドが
含まれます(現在は `1`)。純粋な追加変更(新しいフィールド、新しいenumバリアント)の間はこの値は
変わらず、フィールドの削除・改名があったときにのみ増分されます - そのため、機械側の消費者はフィールド
の有無から推測するのではなく、このバージョン番号でパース方法を切り替えられます。レポート/レコード
ごとのJSON Schemaは [`schemas/`](schemas/) を参照してください。

`compare` のレポートには、`verdict` に影響しない付加的なフィールドも常に含まれます:

* **`estimated_additional_trials`** - `inconclusive` な結果を決着させるのに必要な追加トライアル数のおおまかな見積もり(信頼区間が `O(1/√n)` で縮小するという前提、効果量自体は変わらないと仮定)。提案できることが何もない場合は `null` になります - 既に判定済み、トライアル数が0件、あるいは効果量がpass/failしきい値の"内側"(デッドゾーン)にある場合です: 効果量がすでにデッドゾーン内にある点推定を中心に信頼区間を縮めても、データをどれだけ追加してもどちらの境界も越えられません。この数値は「保証」ではなく「だいたいこのくらい、あるいはもっと必要」という目安として扱ってください - 既知の、定量化されたバイアスがあります(検証済みの一例では n=100 で約18%の過小評価)。
* **`warnings`** - 人間可読なデータ品質の警告で、何もなければ空です: サンプルが小さい(ペアトライアルが30件未満)、失敗率が高い(timeout/crash/invalidが20%超)、`elo` で引き分けが多い(引き分けが50%を超えると、レーティングの根拠となる決着済みの結果が少なくなります)、測定された効果量がCI自身の半値幅より小さい(ゼロ周りのノイズである可能性がある)、またはunpairedモードで同一の`id`が10件以上のid付きトライアル中3回以上繰り返されている(すべての`id`がちょうど2回ずつ出現する場合 - つまり`--paired-by-id`を付け忘れただけのよくあるケース - は発火しません)場合です。
* **`data_quality`** - `warnings` と同じ内容を、文字列ではなく真偽値(`tiny_sample`、`high_failure_rate`、`draw_heavy`、`effect_within_noise_floor`、`low_id_diversity`)として持つフィールドです。文章を解析するのではなくフラグで分岐したい機械側の消費者向けです。`warnings` を置き換えるものではなく併存します - どちらも常に存在します。

各手法の前提・失敗モードの詳細は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

## SPRT

`veridict sprt` は `compare` とは別のモードです。効果量としきい値と照合する信頼区間の代わりに、対数尤度比を累積し、`--alpha`/`--beta` から導かれる2つの境界のいずれかを超えた時点で停止します。`pass` は「候補が少なくとも `--elo1` ポイント強いと確信できる」、`fail` は「高々 `--elo0` ポイントの強さだと確信できる」、`inconclusive` は「データ収集を継続する」ことを意味します。`--alpha`/`--beta` はレポート上の調整可能なつまみではなく、実際に保証される偽陽性率/偽陰性率そのものです。このサブコマンドに `--min-effect`/`--confidence` はありません。2つのバリアントがあります(`--sprt-variant`):

* **`wald`**(デフォルト) - 決着がついた(引き分けを除く)`result` レコードのみに対する古典的な二値SPRT。このモデルの下では引き分けはどちらのElo仮説が正しいかについて情報を持たないため、LLRから完全に除外されます。仮説は `--elo0`/`--elo1` で、標準的なロジスティックEloです。
* **`trinomial`** - 引き分けを考慮した一般化LLR検定(チェスエンジンのテストツール、例えばFishtestで歴史的に使われてきたBayesEloパラメータ化)。引き分け率をプールされた勝ち/引き分け/負けのカウントからニュイサンスパラメータとして推定することで、引き分けの多いデータで `wald` より速く収束します。**単位はロジスティックEloではなくBayesEloです** - この2つは推定された引き分け率がちょうどゼロのときのみ一致するため、`--elo0`/`--elo1` を再解釈するのではなく、別途 `--belo0`/`--belo1` フラグで仮説を与えます。推定された引き分け率(`drawelo`)は、判定に使われているのと同じデータから推定されたものであるため、透明性のため出力に含まれます。

両バリアントの詳細な仕組み(BayesEloとロジスティックEloの単位変換を含む)は [`docs/metrics_ja.md`](docs/metrics_ja.md) を参照してください。

## 比較マトリクス(comparison matrix)

`veridict matrix` は3つ以上の候補を比較し、ペアワイズのElo差を一覧表にします。レポート専用で(判定なし)、成功時は常に終了コード0です: マトリクス全体に対する単一のpass/failは存在しません。データの与え方は2通りあり、1回の実行で自由に組み合わせられます:

* **レガシー方式**: 候補ごとに1ファイル、いずれも*同じ共有ベースライン*に対して測定されたもので、`--metric elo`/`--metric winrate` と同じ `result` フィールドの勝敗/引き分けレコードを使います。
* **`--matches`**(繰り返し指定可): 名前付き対戦相手同士の直接対戦レコード - `{"id": ..., "a": "...", "b": "...", "result": "a_win"|"b_win"|"draw"}` - により、候補同士が共有ベースラインを介さず直接対戦したデータを扱えます。`a`/`b` に文字列 `"baseline"` を指定すると、レガシーファイルが暗黙に持つbaselineノードと接続できます。

得られたグラフがトポロジー的にまだスター形(どの対戦も出どころに関わらずbaselineを含む)であれば、`matrix` は閉形式を使います: 各候補のレーティングはそのままその候補自身のElo-vs-baselineです(スターグラフ上のBradley-TerryのMLEには共同で解くべき共有項が存在しません)。候補同士の実対戦データが存在する場合は、一般Bradley-Terryモデル(グラフ全体に対する反復ソルバー)を使ってフィットします。いずれの場合も、各セルには次のいずれかが付与されます:

* **`direct`** - その行と列の間に実際の直接対戦データが存在する。
* **`inferred`**(Markdown表では `*`)- 両者ともレーティングは付いており比較可能だが、直接対戦したことはない - モデルによる外挿 `elo_i - elo_j`。
* **`disconnected`**(Markdown表では `n/a`)- 両者を結ぶ経路が存在しない(例: 共通の対戦相手を持たない2つの独立した対戦クラスタ)。この場合、両者の間には不確実なレーティング差があるのではなく、有限のレーティング差そのものが存在しません - `elo_diff` は推測値ではなく `null` です。

スターグラフ/レガシー方式のセルは従来どおり実際のWilson区間を保持します。一般グラフモードのマトリクスセル(`direct`/`inferred`)も、実際のブートストラップ信頼区間を得られるようになりました: 各リサンプルではすべての対戦カードの勝敗/引き分けの実測比率からタリーを引き直し、グラフ全体を再フィットします。`ci_low`/`ci_high` は、そのペアが同じコンポーネントに留まったリサンプルにおける `elo_i - elo_j` から得られます。`matrix` の `--resamples`(デフォルト2,000)、`--seed`、`--bootstrap-method percentile`(デフォルト)/`basic`/`bca`(`compare` の同名フラグと同じ3手法・同じ意味)でこれを制御できます(いずれもスターグラフモードでは無視され、閉形式のWilson区間のままです)。`elo_diff` が付いているのに `ci_low`/`ci_high` が `null` のままのセルもあり得ます - これは実測データ上は繋がっているものの、リサンプリングに対してその接続が脆弱すぎる(リサンプルの90%未満でしか同じコンポーネントに留まらない)ため、誤って狭い区間を報告するよりは「信頼区間なし」と明示することを選んだ結果です。`CandidateSummary` 自体の `ci_low`/`ci_high` は、一般グラフモードでは引き続き常に `null` です: 個々のレーティングはそのコンポーネント内の任意の基準対戦相手との相対値でしかなく、`elo_i - elo_j` の信頼区間とは異なり、それ自体に信頼区間を付けると誤解を招くためです。

## ペアテストケース(paired testcases)

`--paired-by-id`(`compare`、`sprt`、`matrix` で使用可能)は、同じ `id` を持つ2つのレコードを「同じテストケースを2回実行したもの」(例: そのテストケース固有のバイアスを打ち消すために役割を入れ替えて再実行したもの)とみなし、2つの独立した観測ではなく1つの正味の観測として結合します:

* `winrate`/`elo`: ペア全体の合計ポイント(勝ち=1、引き分け=0.5、負け=0という、いわゆる「ペアゲーム」の標準的な採点方式)で正味化します - 合計が`1`より大きければ正味candidate勝ち、`1`未満なら正味baseline勝ち、ちょうど`1`なら正味引き分けです。
* `mean-diff`/`sign-test`: ペアの2つの差分の平均で正味化します。

`id` が1回しか出現しない場合は通常のペアなしサンプルとして扱われます(1つのファイルにペアありとペアなしのテストケースが混在しても問題ありません)。同じ `id` を持つレコードが3つ以上ある場合は、ペアへ黙って切り詰めるのではなく、データエラーとして拒否されます。`--paired-by-id` を指定しない場合、`mean-diff`/`sign-test` レコードの `id` 重複はこのフラグの有無に関わらず従来どおり拒否されます。

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
* **`mean-diff` の信頼区間** - percentile / BCa(バイアス補正・加速)ブートストラップ。いずれも
  Efron & Tibshirani『An Introduction to the Bootstrap』(1993年、14章)に基づきます。
* **`elo`** - ロジスティックElo モデル。Eloの原型のレーティングシステム(Elo 1978)を、広く使われて
  いる形に変形したものです。
* **`sprt`** - Waldの逐次確率比検定(Wald 1945)。
* **`matrix` の一般グラフモード** - Bradley-Terryのペア比較モデル(Bradley & Terry 1952)を、
  Zermelo(1929)/Hunter(2004)のMM(Minorization-Maximization)不動点法でフィットします。有限な解が
  存在するための条件はFord(1957)によります。

一方で、次の値は学術論文由来の厳密な統計的結果ではありません - 本プロジェクト独自の設計判断・経験則
であり、それを定理であるかのように装ってはいません:

* **`pass`/`fail`/`inconclusive`** - 信頼区間を閾値と比較すること自体は標準的な決定則ですが、どの
  閾値を使うか、および「false passはinconclusiveより悪い」という保守的な方針(判定ロジック参照)は、
  本プロジェクト独自の設計判断です。
* **`estimated_additional_trials`** - `winrate`/`sign-test`/`elo` では、レポートが実際に使っている
  CI計算式に対する二分探索であり、想定モデル(点推定を固定)のもとでは厳密です。例外は`mean-diff`で、
  ブートストラップCIにはそのような閉形式が存在しないため、`O(1/sqrt(n))` のスケーリングによる近似に
  フォールバックします。これには既知のバイアスがあります(レポートの追加情報を参照)。
* **`warnings`** - サンプル数30件・失敗率20%・引き分け率50%といった閾値は、特定の論文由来ではなく
  慣習的な経験則です。

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
