# メトリクス・リファレンス

[English](metrics.md) | 日本語

この文書は公開版`0.19.6`の仕様を定めます。対応するソースは
[v0.19.6 tag](https://github.com/kent-tokyo/veridict/tree/v0.19.6)に残しています。
使用例は[README](../README_ja.md)、未実装の候補は[リサーチマップ](research-map_ja.md)を参照してください。

## 判定契約

効果量の信頼区間を `[L, U]` とすると、判定は次のとおりです。

- `L >= pass_above` なら `pass`。
- `U <= fail_below` なら `fail`。
- それ以外は `inconclusive`。

`--min-effect x` は `pass_above = x`、`fail_below = -x` の対称指定です。複数メトリクスで単位が
異なる場合は `metric=value` を使います。入力や設定の不備は統計判定ではなく、終了コード `3` です。

`promotion` は証拠の妥当性も含む昇格判定です。妥当性上限を超えると統計的な `pass` を
`inconclusive` に下げますが、新たな `pass` や `fail` は作りません。

## 固定標本メトリクス

| メトリクス | 1観測 | 効果量 | 区間 |
|---|---|---|---|
| `winrate` | 決着した勝敗 | 候補勝率から `0.5` を引いた値 | Wilson、exact、Jeffreys |
| `sign-test` | 同値でない対応スコア | 正符号率から `0.5` を引いた値 | Wilson、exact、Jeffreys |
| `mean-diff` | 対応スコア | `candidate - baseline` の平均 | percentile、basic、BCaブートストラップ |
| `quantile-diff` | 対応スコア | 対応差の指定分位点 | percentile、basicブートストラップ |
| `relative-diff` | `baseline > 0` の対応スコア | レコードごとの比例変化の平均 | percentile、basic、BCaブートストラップ |
| `elo` | 勝ち・引き分け・負けの得点 | 平均得点のロジスティックElo | 得点区間の変換 |
| `paired-elo` | 完了した2試行ペア | ペア平均得点のロジスティックElo | ペア単位のpercentileブートストラップ |

### メトリクス固有の規則

- `winrate` は引き分け、`sign-test` は同値を除外します。`--ci-method` は `wilson`（既定）、
  Clopper-Pearsonの `exact`、`jeffreys` から選びます。決着した観測がなければ `inconclusive` です。
- `mean-diff` は対応差を再標本化し、2列を別々には再標本化しません。
- 数値入力だけでなく、対応差・効果量・区間の各端点も有限値でなければなりません。表現できない値が
  生じた場合は終了コード `3` の入力・設定エラーとし、JSONの値を `null` にしたまま `pass` には
  しません。意味を変えない範囲で入力値と閾値の尺度を調整してください。
- `quantile-diff` の既定値は中央値です。自動処理では `--quantile` を明示してください。非平滑な
  標本分位点に対して現在の較正で利点を確認できていないため、BCaは拒否します。
- `relative-diff` は `(candidate - baseline) / baseline` の平均です。`tied_count` と
  `data_quality.diluted_by_ties` は診断であり、判定を変えません。
- ブートストラップでは `--resamples` と再現可能な `--seed` を使います。既定は `percentile`、
  `basic` は点推定を中心に端点を反転し、BCaはバイアスと加速を補正します。
- `mean-diff` は基準値のスケール幅を警告する場合があります。絶対差と比例差のどちらを主張するかは、
  結果を見る前に決めてください。警告は判定には影響しません。

### Elo

候補勝ちは `1`、引き分けは `0.5`、基準側の勝ちは `0` とします。平均得点を `p` とすると:

```text
Elo = 400 * log10(p / (1 - p))
```

変換前に得点を `[0.0001, 0.9999]` へ制限するため、全勝・全敗でも有限値（約 `+/-1600` Elo）です。
このEloは観測対象の結果尺度であり、その外側での強さを証明しません。

`paired-elo` には `--paired-by-id`、全レコードの空でないID、IDごとに利用可能な結果がちょうど2件
必要です。ペア合計 `{0, 0.5, 1, 1.5, 2}` を `pair_score_counts` に保持し、2で割ってEloへ変換し、
ペア全体を再標本化します。ID欠落、1件だけのID、3件以上のID、不完全な結果は入力エラーです。

`paired-elo` は固定標本分析です。五項SPRTの代替ではありません。従来の
`elo --paired-by-id` は、引き続き各ペアを正味の勝ち・引き分け・負けにまとめます。

## ペア化、クラスタリング、失敗

`--paired-by-id` は選択したメトリクスの規則でレコードをまとめます。通常は1件なら単独観測、2件なら
ID単位の1観測、3件以上なら不正です。勝敗メトリクスは2件の得点を正味化し、数値メトリクスは2件の
対応差を平均します。`paired-elo` だけは上記の完了ペアを必須とします。

`--cluster-by-id` は全行を残し、IDクラスター全体を再標本化します。`winrate` と `elo` だけに対応し、
十分なクラスター数が必要です。`--paired-by-id` とは併用できません。

ステータスは `ok`、`timeout`、`crash`、`invalid` です。

| `--failure-policy` | 勝敗への反映 |
|---|---|
| `report-only` | 失敗を数え、明示された結果があれば使う |
| `exclude` | どちらかが失敗したレコードの結果を除外する |
| `loss` | 片側失敗をその側の負け、両側失敗を引き分けにする |

失敗件数は常に報告します。`exclude` と `loss` は勝敗メトリクスと逐次検定専用です。
`--max-timeouts`、`--max-crashes`、`--max-invalid` は妥当性上限で、超過すると昇格できません。

## 複数メトリクス

`--metric` を繰り返すと複数の主張を評価します。全体判定の優先順位は
`fail > inconclusive > pass` です。

`--claim-correction` は通常の `verdict` や `promotion` を置き換えず、一群補正項目を追加します。

- `none`: 補正なし。
- `bonferroni`: alphaを均等配分。
- `holm`: 順序付きHolm補正。

ブートストラップメトリクスまたは `--cluster-by-id` を含む一群は、対応する補正区間を二項分布の閉形式
から再構成できないため拒否します。

## 逐次検定

### 判定規則とバリアント

`sprt` は単純仮説を次の境界で比較します。

```text
upper = ln((1 - beta) / alpha)
lower = ln(beta / (1 - alpha))
```

両方とも `(0, 1)` の範囲で、かつ `alpha + beta < 1` でなければなりません。下側境界が
上側境界以上になる設定はエラーとして拒否します。
仮説値は表現可能な確率に変換でき、両方の境界が有限値でなければなりません。LLRまたはdraw-rateの
中間値が非有限になった場合は、数値フィールドがJSON `null` の判定を返さず、終了コード `3` の
設定エラーにします。

上側境界で `pass`、下側境界で `fail`、どちらにも達しなければ `inconclusive` です。

| バリアント | データと仮説 |
|---|---|
| `wald` | 決着した勝敗。`--elo0` と `--elo1` のロジスティックElo |
| `trinomial` | 勝ち・引き分け・負けの一般化LLR。`--belo0` と `--belo1` のBayesElo |
| `pentanomial` | 完了ペア得点 `{0, 0.5, 1, 1.5, 2}`。ロジスティックEloと `--paired-by-id` |

すべてのバリアントが入力順に再生し、最初の有効な境界到達を保持します。`wald`と`trinomial`は利用可能な
試行ごと、ペア化時は正味ペアの完了ごとに評価し、`pentanomial`は完了ペアごとに評価します。停止後の
レコードは判定時点の値を変えませんが、入力検証と失敗上限の集計対象です。
`--min-paired-ids`、`--max-paired-ids`、`--require-complete-pairs` は、対応するペアモードを制約します。

### レポート項目と入力の再生

判定には `verdict`、`promotion`、`decision_*` を使います。`available_*`、`analyzed_*`、`stopping_*`、
`ignored_*_after_stop` は再生状況を示します。schema v1との互換性のため、`wald`と`trinomial`では、
接頭辞のない件数と `llr` が入力全体の値を表します。これらから判定を再構成しないでください。

SPRTレポートは、非ペアモードでID付きレコードが10件以上あり、同じIDが3回以上現れた場合に、
`data_quality.low_id_diversity` と対応する `warnings` を出します。判定材料はIDの構造だけで、
LLR、`verdict`、`validity`、`promotion` は変えません。各IDがちょうど2回ずつ現れる場合と、
`--paired-by-id` の使用時は、重複IDにペアとしての意味があるため警告しません。

### ライブラリAPIとチェックポイント

ライブラリからは、`SprtRunOptions` を渡す
`sprt::run_with_options(records, config, options)` を推奨します。既定値は`wald`、非ペア、
`FailurePolicy::ReportOnly`、不完全IDを許容、ペアID上下限なしです。従来の位置引数版
`sprt::run` も互換ラッパーとして残り、同じ検証経路を使います。

長時間動くデータ生成側では、`sprt::SprtAccumulator` にレコードを追加し、`snapshot()` が返す
`SprtCheckpoint` を保存できます。再開時は、生成側が別に保存したカーソルを `expected_position` として
`SprtAccumulator::from_checkpoint(checkpoint, config, options, expected_position)` に渡します。

チェックポイントの版、仮説、オプション、レコード数、最終行、先頭部分のフィンガープリントが異なる
場合は復元を拒否します。`push` の行番号は入力順に増加させてください。最初の境界到達、不完全ペア、
失敗集計は再開後も維持します。フィンガープリントは入力の取り違えを検出するもので、認証には使えません。
失敗上限はチェックポイントのオプションに含まれません。CLIと同じ妥当性・昇格判定が必要な場合は、
完了した一括レポートと再開レポートへ同じ `FailureCaps` を適用してください。

## 補助コマンド

### `matrix` と `plan`

`matrix` は連結成分ごとにBradley-Terry強度を推定し、候補間のロジスティックEloを報告します。
異なる成分間は `disconnected` とnull推定値です。記述的な出力で、全体判定はありません。

`plan` は必須の `--min-elo` に対し、追加証拠が必要な比較を順位付けします。最適化器、スケジューラー、
試行実行器ではありません。

### `power`

`power` はデータ収集前の必要証拠量を見積もります。

- `winrate` と `sign-test`: 選択した区間に対する厳密探索。
- `elo`: 二項得点の探索。引き分けが多い場合は下限として扱う。
- `mean-diff`: `--assume-sd` または `--pilot` を使う正規近似。
- `--sprt`: Waldの平均標本数近似。

`paired-elo`、`relative-diff`、`quantile-diff` には未対応です。検出力の出力は計画値であり、判定や
必要標本数の保証ではありません。

### `verify-run`

`verify-run` はペア、順序、データ混入と、manifest値と記録した不透明な識別子・ハッシュの整合性を
検証します。成果物の読み込み、ハッシュの再計算、ベンチマーク品質の証明は行いません。

### `time-sensitive`

`time-sensitive` は、ベルヌーイ分布の2つの単純仮説を比べる実験的な片側検定です。有限の報酬期間を持ち、
任意の停止時点でも妥当性を保ちます。
方策は `bellman`、`edo`、`gro`、報酬は `hard-deadline`、`exponential`、カスタムスケジュールです。
引き分けはwealthを変えずに時間だけ進めます。Elo、引き分けを分けた仮説、複合仮説のオンライン推定には
対応しません。

## レポートの読み方

- `estimated_additional_trials` は固定標本の未決着結果に対する参考値です。ブートストラップでは
  `O(1/sqrt(n))` の区間幅近似を使い、`null` になる場合があります。
- `inconclusive_kind` は、区間がゼロを除外すれば `directional`、ゼロをまたげば `noise`、通常の
  区間判定が成立しなければ `null` です。
- `warnings` は小標本、失敗、引き分け、疎な裾、広いスケールなどの解釈リスクを示します。
  通常は判定を変えません。

### 小標本の下限

固定標本レポートでは、独立した分析単位が2件未満なら `inconclusive` にします。1件だけでは経験的な
不確実性を推定できないためです。`paired-elo`では完成ペア、その他のペアモードではIDグループまたは
IDなしの観測、クラスタ分析ではクラスタを1単位と数えます。30件未満では引き続き警告します。
2件あれば小標本の誤判定率が保証される、という意味ではありません。

[`schemas/`](../schemas/) のJSON Schemaは、記載された入出力形式を対象とします。ただし、実験的な
`time-sensitive` レポートにはまだ対応スキーマがありません。現在のソースツリーのSPRTレポートは、
`warnings`と`data_quality`を必須にしたschema v2です。
[`sprt-report-v1.schema.json`](../schemas/sprt-report-v1.schema.json) は、これらの項目を追加する前の厳格な
形を保存します。公開版`0.19.5`はversion番号1のまま両項目を出力したため、この厳格な旧スキーマには
適合しません。現在のソースツリーにv1出力モードはありません。履歴データを検証するときは、生成元の
版を固定したうえで `schema_version` に対応するスキーマを選んでください。

## 参考文献

- Wilson, E. B. (1927). “Probable Inference, the Law of Succession, and Statistical Inference.”
- Clopper, C. J.; Pearson, E. S. (1934). “The Use of Confidence or Fiducial Limits Illustrated in
  the Case of the Binomial.”
- Efron, B.; Tibshirani, R. J. (1993). *An Introduction to the Bootstrap*.
- Wald, A. (1945). “Sequential Tests of Statistical Hypotheses.”
- Elo, A. (1978). *The Rating of Chessplayers, Past and Present*.
- Bradley, R. A.; Terry, M. E. (1952). “Rank Analysis of Incomplete Block Designs: I.”
- Hunter, D. R. (2004). “MM Algorithms for Generalized Bradley-Terry Models.”
