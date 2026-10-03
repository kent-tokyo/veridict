# メトリクス・リファレンス

[English](metrics.md) | 日本語

この文書は、実装済みコマンドの統計仕様を定めます。基本的な使い方は
[README](../README_ja.md)、未実装の候補は[リサーチマップ](research-map_ja.md)を参照してください。

## 共通の判定規則

信頼区間を `[L, U]` とすると、判定は次のとおりです。

- `L >= pass_above` なら `pass`。
- `U <= fail_below` なら `fail`。
- それ以外は `inconclusive`。

`--min-effect x` は、`pass_above = x`、`fail_below = -x` の対称指定です。複数メトリクスの単位が
異なる場合は、`metric=value` 形式でしきい値を指定してください。入力や設定の不備は統計的な判定では
なく、終了コード `3` です。

`promotion` は `verdict` より厳しい昇格判定です。妥当性チェックは統計的な `pass` を
`inconclusive` に下げることはありますが、`pass` や `fail` を新たに作りません。

## 比較メトリクス

| メトリクス | 1観測 | 点推定 | 区間 |
|---|---|---|---|
| `winrate` | 決着した `result` | 候補勝率から `0.5` を引いた値 | Wilson、exact、Jeffreys |
| `sign-test` | 対応のある数値 | 正の符号の割合から `0.5` を引いた値 | Wilson、exact、Jeffreys |
| `mean-diff` | `candidate - baseline` | 算術平均 | percentile、basic、BCaブートストラップ |
| `quantile-diff` | `candidate - baseline` | 指定した標本分位点 | percentile、basicブートストラップ |
| `relative-diff` | `(candidate - baseline) / baseline` | 算術平均 | percentile、basic、BCaブートストラップ |
| `elo` | 勝ち・引き分け・負けの得点 | 平均得点のロジスティックElo変換 | 得点区間の変換 |

### `winrate` と `sign-test`

`winrate` は引き分け、`sign-test` は数値が同じペアを除外します。効果量と区間の両端は、比率から
`0.5` を引いてゼロ中心にします。そのため `--min-effect 0.02` は、決着時の候補勝率または正符号率が
52%であることを意味します。`baseline_count` と `candidate_count` は各側の決着数、`paired_count` は
その合計です。

`--ci-method` で次の区間を選びます。

- `wilson`（既定）: Wilsonスコア区間。
- `exact`: Clopper-Pearson区間。
- `jeffreys`: Jeffreys事前分布を使うbeta事後分布の等裾区間。

ExactとJeffreysは整数の二項カウントを必要とします。決着した観測がなければ
`inconclusive` です。

### `mean-diff`

各レコードが1つの対応差を生成します。ブートストラップでは差を再標本化します。`baseline` 列と
`candidate` 列を別々に再標本化することはありません。`--resamples` で反復数、`--seed` で再現可能な
乱数シードを指定します。

既定は `percentile` です。`basic` は点推定を中心に `percentile` 区間を反転します。`bca` はバイアスと
加速を補正し、ジャックナイフ項を計算できる十分な非退化データを必要とします。

### `quantile-diff`

レコードごとの対応差について、指定した標本分位点を求めます。既定は `0.5`（中央値）です。自動処理では
`--quantile` を明示してください。

このメトリクスではBCaを受け付けません。標本分位点は非平滑で、現在の較正試験では採用を正当化できるほどの
被覆率改善を確認できなかったためです。`percentile` または `basic` を使用します。

### `relative-diff`

利用する全レコードで `baseline > 0` が必要です。効果量はレコードごとの比率の平均であり、集計平均の
比ではありません。`0.03` は平均3%の相対増加を表します。

`tied_count` は差が厳密にゼロの件数です。`data_quality.diluted_by_ties` は、一部のケースだけに効く
変更が未変更ケースによって薄まっている可能性を示します。この診断はメトリクスや判定を変えません。

### スケール診断

`mean-diff` の `scale_diagnostics` は基準値の大きさだけを調べます。スケール幅の警告は、大きなケースが
絶対差の平均を支配する可能性を示します。判定には影響しません。比例変化を主張したい場合は、結果を見る
前に `relative-diff` を選んでください。

### `elo`

候補勝ちは `1`、引き分けは `0.5`、基準側の勝ちは `0` とします。平均得点を `p` とすると、点推定は
次のロジスティックElo変換です。

```text
Elo = 400 * log10(p / (1 - p))
```

区間の両端も同じ式で変換します。境界得点ではEloが無限大になるため、JSONでは非有限の端点を `null` とし、
警告を追加します。このEloは観測した母集団に対する結果尺度であり、母集団外の強さを証明しません。

## ペア化とクラスタリング

`--paired-by-id` は、同じIDのレコードをメトリクスごとの規則でまとめます。1件だけのIDはそのまま使い、
2件ならID単位の1観測にし、3件以上はエラーです。勝敗メトリクスは2件の得点を正味化し、数値メトリクスは
2件の対応差を平均します。ペア化しない場合、一意性が必要なメトリクスでは重複IDを拒否します。

`--cluster-by-id` は別機能です。全試行を残し、`winrate` または `elo` についてIDクラスター単位で
再標本化します。相関のある行を独立とみなすことを防ぎます。十分なクラスター数が必要で、
`--paired-by-id` とは併用できません。数値メトリクスには未対応です。

## 失敗と証拠の妥当性

`baseline_status` と `candidate_status` は `ok`、`timeout`、`crash`、`invalid` を受け付けます。

| `--failure-policy` | メトリクスでの扱い |
|---|---|
| `report-only` | 失敗を数え、明示された `result` があれば使用する |
| `exclude` | どちらかが失敗したレコードの結果を使わない |
| `loss` | 候補失敗は基準側の勝ち、基準側の失敗は候補勝ち、両側失敗は引き分けとする |

どの方針でも失敗件数は隠しません。`exclude` と `loss` は勝敗メトリクスと逐次検定だけで利用できます。
数値メトリクスで指定すると設定エラーです。

`--max-timeouts`、`--max-crashes`、`--max-invalid` は妥当性の上限です。上限を超えると理由を記録し、
昇格を止めます。複数メトリクスでは、メトリクス別と全体の両方に妥当性と昇格可否を記録します。

## 複数メトリクスと主張の補正

`--metric` を繰り返すと、同じ入力から複数の主張を評価します。全体判定の優先順位は
`fail > inconclusive > pass` です。

`--claim-correction` は、複数の主張をまとめたときの誤判定率を調整します。

- `none`: 各メトリクスを指定信頼度で独立に評価。
- `bonferroni`: 主張数でalphaを等分。
- `holm`: 順序付きHolm補正。同じ一群ではBonferroni以上の検出力。

補正は通常の `verdict` や `promotion` を置き換えません。別項目の
`family_adjusted_verdict`、`family_adjusted_promotion`、複数レポートの
`simultaneous_claims_promotion` に記録します。補正後の主張は維持または厳格化されるだけです。
ブートストラップメトリクスまたは `--cluster-by-id` を含む一群は、二項の閉形式から補正区間を再構成
できないため設定エラーです。

## SPRT

`sprt` は、2つの単純仮説を対数尤度比の境界で比較します。

```text
upper = ln((1 - beta) / alpha)
lower = ln(beta / (1 - alpha))
```

上側境界を超えると `pass`、下側境界を下回ると `fail`、どちらにも達しなければ
`inconclusive` です。

### バリアント

- `wald`: 決着した結果を使う二値検定。仮説はロジスティックElo（`--elo0`、`--elo1`）。
- `trinomial`: 引き分けのニュイサンスパラメータを推定する勝ち・引き分け・負けの一般化LLR。仮説は
  BayesElo（`--belo0`、`--belo1`）であり、ロジスティックEloではありません。
- `pentanomial`: 2試行のペア得点 `{0, 0.5, 1, 1.5, 2}` を使う一般化LLR。
  `--paired-by-id` と、完了ペアごとにちょうど2レコードが必要です。

すべてのバリアントが入力を順番に再生し、最初の有効な境界超過を保持します。入力順は実験スケジュールの
一部です。`wald` と `trinomial` は利用可能な試行ごと、`--paired-by-id` 指定時は正味ペアの完了ごとに評価し、
`pentanomial` はペア完了ごとに評価します。停止後のレコードは判定時点のLLRや勝敗件数を変えませんが、
入力検証と失敗集計の対象です。失敗上限を超えると最終判定は `inconclusive` になります。
`--min-paired-ids` と `--max-paired-ids` は `pentanomial` の評価範囲を制限します。
`--require-complete-pairs` は、対応するモードで未完了ペアを拒否します。

### レポート互換性

判定時点の値は、`decision_llr`、`decision_candidate_wins`、
`decision_baseline_wins`、`decision_draws` と、共通の `available_*`、`analyzed_*`、
`stopping_observation_*`、`ignored_*_after_stop` で表します。

schema v1との互換性を保つため、Wald/trinomialの接頭辞なし `llr` と勝敗件数は入力全体を表します。
Pentanomialは従来どおり解析プレフィックスを表します。従来の `llr` から判定を再構成せず、
`verdict`、`promotion`、`decision_*` を使用してください。

## `matrix` と `plan`

`matrix` は、共通の基準に対する候補群、または `--matches` で一般的な直接対戦グラフを受け付けます。
連結成分ごとにBradley-Terry強度を推定し、候補間のロジスティックElo差と区間を報告します。異なる成分間の
セルは強度を比較できないため、`status=disconnected` とnull推定値を返します。`matrix` は記述的な出力で、
全体の `pass`/`fail` 判定はありません。

`plan` は同じグラフと必須の `--min-elo` を使い、追加の証拠が必要な比較を順位付けします。最適化器や
スケジューラーではありません。

## 検出力計算（`power`）

`power` は、データ収集前に必要な証拠量を見積もります。

- `winrate` と `sign-test`: 選択した二項区間に対する厳密探索。
- `elo`: 二項得点をEloへ変換する探索。引き分けをモデル化しないため、引き分けが多い場合は下限として
  扱います。
- `mean-diff`: `--assume-sd` または `--pilot` の対応差から推定した標準偏差を使う正規近似。
- `--sprt`: 各仮説におけるWaldの平均標本数近似。境界を越えた分を無視するため、上限ではなく期待値です。
  `--horizon` を指定すると、その試行数まで未決着である確率を固定シードのMonte Carlo法で見積もります。

`relative-diff` と `quantile-diff` の検出力計算は未実装です。割合メトリクスの
`--paired-by-id` は注意事項を追加しますが、データ収集前に相関を推定することはできません。
`mean-diff` の事前データでは実際のペア化規則を適用します。計画用の入力は確認実験より前に固定します。
見積もりは判定ではありません。

## 時間価値付き検定

`time-sensitive` は、任意の停止時点で妥当性を保つ、実験的なBernoulli単純仮説間の片側検定です。
有限の期間内で固定した `p0` と `p1` を比較し、宣言した時間価値の報酬を最適化します。

方策:

- `bellman`: 設定した報酬に対する動的計画法。
- `edo`: 期待割引の定常近似。`exponential` 報酬でのみ利用可能。
- `gro`: 成長率を重視する基準方策。報酬スケジュールは使わない。

報酬は `hard-deadline`、`exponential`、または `--reward-schedule FILE` で指定します。引き分けでも
試行数は進みますが、`wealth` は変わりません。そのため、計画上の報酬は全試行が決着する場合を仮定します。
Elo仮説、引き分けを別確率として扱う三項仮説、複合仮説のオンライン推定には対応しません。終了コードは
`pass` が `0`、未決着が `2`、
入力または設定エラーが `3` です。

## レポートの補助項目

### `estimated_additional_trials`

固定標本メトリクスが `inconclusive` の場合に、現在の効果が続くと仮定して追加観測数を見積もります。
閉形式がある場合はそれを使い、ブートストラップでは区間幅が `O(1/sqrt(n))` で縮む近似を使います。
参考値であり、現在の効果がしきい値のデッドゾーン内にある場合などは `null` になります。

### `inconclusive_kind`

- `directional`: 区間はゼロを除外するが、判定しきい値を超えない。
- `noise`: 区間がゼロをまたぐ。
- `null`: 利用可能な観測がゼロ、妥当性上限超過など、通常の区間判定ではない。

### `warnings`

警告は、小標本、高い失敗率や引き分け率、対象分位点付近のデータ不足、基準値のスケール幅、Elo端点の
非有限値など、既知の解釈リスクを示します。透明性のための目安であり、判定を変えません。

## 参考文献

- Wilson, E. B. (1927). “Probable Inference, the Law of Succession, and Statistical Inference.”
- Clopper, C. J.; Pearson, E. S. (1934). “The Use of Confidence or Fiducial Limits Illustrated in
  the Case of the Binomial.”
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*.
- Wald, A. (1945). “Sequential Tests of Statistical Hypotheses.”
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*.
- Bradley, R. A.; Terry, M. E. (1952). “Rank Analysis of Incomplete Block Designs: I.”
- Hunter, D. R. (2004). “MM Algorithms for Generalized Bradley-Terry Models.”
