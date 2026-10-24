//! Fixed-sample comparison orchestration and report construction.

use super::*;

/// Lets `compare_one`/`compare_many` (and `metrics::compute`/
/// `metrics::compute_many`) accept either a streaming `Result`-yielding
/// iterator (the CLI's real use case - parsing JSONL/CSV can fail mid-
/// stream) or a plain iterator over already-valid `(usize, Record)` pairs (a
/// caller that already has a validated slice/`Vec` in memory, with no
/// `Result` to thread through) through the *same* function, instead of
/// forcing every in-memory caller to write `.map(Ok)` just to satisfy the
/// type. A blanket `impl<T> From<T> for Result<T, E>` isn't available here
/// (implementing a foreign trait, `From`, for a foreign type, `Result` -
/// even parameterized by a local type - violates Rust's orphan rule), hence
/// this small local trait instead.
pub trait IntoRecordResult {
    fn into_record_result(self) -> Result<(usize, input::Record), VeridictError>;
}

impl IntoRecordResult for Result<(usize, input::Record), VeridictError> {
    fn into_record_result(self) -> Self {
        self
    }
}

impl IntoRecordResult for (usize, input::Record) {
    fn into_record_result(self) -> Result<(usize, input::Record), VeridictError> {
        Ok(self)
    }
}

/// Runs one metric end to end: classify records, compute its effect and
/// confidence interval, and apply the pass/fail thresholds. `paired_by_id`
/// enables paired-testcase variance reduction (see `metrics::compute`).
/// `cluster_by_id` enables a cluster-robust bootstrap CI instead (winrate/elo
/// only - mutually exclusive with `paired_by_id`, see
/// `VeridictError::IncompatibleClusterById`/`ClusterByIdConflictsWithPairedById`).
#[allow(clippy::too_many_arguments)]
pub fn compare_one<I>(
    records: I,
    metric: MetricConfig,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<Report, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    let out = metrics::compute(
        records,
        metric,
        confidence,
        resamples,
        seed,
        paired_by_id,
        cluster_by_id,
    )?;
    Ok(build_report(
        metric.kind(),
        confidence,
        thresholds,
        out,
        metric.ci_method(),
        paired_by_id,
    ))
}

/// Runs several metrics against the same records in a single pass over
/// `records` (see `metrics::compute_many`), each against its own threshold
/// (`thresholds[i]` for `metrics[i]` - a per-metric CLI override, or the same
/// value repeated if the caller wants one threshold shared across all of
/// them), and combines them into one overall verdict: `Fail` if any metric
/// fails, else `Inconclusive` if any metric is inconclusive, else `Pass`.
/// Matches the "a false pass is worse than an inconclusive result" rule: one
/// metric failing sinks the whole run.
#[allow(clippy::too_many_arguments)]
pub fn compare_many<I>(
    records: I,
    metrics: &[MetricConfig],
    confidence: f64,
    thresholds: &[verdict::Thresholds],
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<MultiReport, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    if metrics.len() != thresholds.len() {
        return Err(VeridictError::InvalidThreshold(format!(
            "metric/threshold count mismatch: {} metric(s), {} threshold configuration(s)",
            metrics.len(),
            thresholds.len()
        )));
    }
    let outs = metrics::compute_many(
        records,
        metrics,
        confidence,
        resamples,
        seed,
        paired_by_id,
        cluster_by_id,
    )?;
    let reports: Vec<Report> = metrics
        .iter()
        .zip(thresholds)
        .zip(outs)
        .map(|((&config, thresholds), out)| {
            build_report(
                config.kind(),
                confidence,
                thresholds,
                out,
                config.ci_method(),
                paired_by_id,
            )
        })
        .collect();
    let verdict = verdict::aggregate(reports.iter().map(|r| r.verdict));
    Ok(MultiReport {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion: Promotion::decide(Validity::Valid, verdict),
        simultaneous_claims_promotion: None,
        reports,
    })
}

fn build_report(
    metric: MetricKind,
    confidence: f64,
    thresholds: &verdict::Thresholds,
    out: metrics::MetricOutput,
    ci_method: CiMethod,
    paired_by_id: bool,
) -> Report {
    let independent_units = out.cluster_count.unwrap_or(out.paired_count);
    let below_decisive_floor = out.warning.is_none() && independent_units < MIN_DECISIVE_UNITS;
    let analysis_unit = if out.cluster_count.is_some() {
        "cluster"
    } else if metric == MetricKind::PairedElo {
        "pair"
    } else if paired_by_id {
        "paired unit"
    } else {
        "trial"
    };
    // Zero usable trials means "no signal", not "the CLI ran a threshold
    // check on a fabricated zero": force Inconclusive rather than letting
    // (0.0, 0.0) accidentally satisfy a threshold that includes zero.
    let (verdict, reason) = match (&out.warning, below_decisive_floor) {
        (Some(warning), _) => (Verdict::Inconclusive, warning.clone()),
        (None, true) => (
            Verdict::Inconclusive,
            format!(
                "Only {independent_units} independent {analysis_unit}(s) were available; at least \
                 {MIN_DECISIVE_UNITS} are required for a decisive fixed-sample verdict."
            ),
        ),
        (None, false) => verdict::decide(out.ci_low, out.ci_high, thresholds),
    };
    // `None` for `Pass`/`Fail`, and for the `out.warning.is_some()` zero-trial path above (where
    // ci_low == ci_high == 0.0 would misleadingly read as `Noise` - it's a data-availability
    // problem, not a real CI judgment). `apply_failure_caps` clears this the same way it clears
    // `estimated_additional_trials` below, for the same reason.
    let inconclusive_kind =
        if verdict == Verdict::Inconclusive && out.warning.is_none() && !below_decisive_floor {
            Some(verdict::classify_inconclusive(out.ci_low, out.ci_high))
        } else {
            None
        };
    // `estimate_additional_trials` binary-searches wilson/jeffreys/exact -
    // none of which describe a cluster bootstrap CI's width at a hypothetical
    // n, and the independent unit under clustering is the cluster, not the
    // trial, so `paired_count` isn't even the right n to scale from.
    let estimated_additional_trials = if out.cluster_count.is_some() || below_decisive_floor {
        None
    } else {
        verdict::estimate_additional_trials(
            metric,
            ci_method,
            verdict,
            out.effect,
            out.ci_low,
            out.ci_high,
            out.paired_count,
            thresholds,
            confidence,
        )
    };
    let (data_quality, warnings) = collect_data_quality(metric, &out, paired_by_id);
    let promotion = Promotion::decide(Validity::Valid, verdict);

    Report {
        schema_version: report::REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion,
        metric,
        baseline_count: out.baseline_count,
        candidate_count: out.candidate_count,
        paired_count: out.paired_count,
        effect: out.effect,
        confidence,
        ci_low: out.ci_low,
        ci_high: out.ci_high,
        pass_above: thresholds.pass_above,
        fail_below: thresholds.fail_below,
        timeouts: out.timeouts,
        crashes: out.crashes,
        invalid: out.invalid,
        failure_breakdown: out.failures,
        reason,
        estimated_additional_trials,
        inconclusive_kind,
        warnings,
        data_quality,
        quantile: out.quantile,
        cluster_count: out.cluster_count,
        max_cluster_size: out.max_cluster_size,
        effective_sample_size: out.effective_sample_size,
        design_effect: out.design_effect,
        scale_diagnostics: out.scale_diagnostics,
        tied_count: out.tied_count,
        pair_score_counts: out.pair_score_counts,
        correction_method: None,
        family_size: None,
        achieved_alpha: None,
        adjusted_alpha_threshold: None,
        unadjusted_verdict: None,
        family_adjusted_verdict: None,
        family_adjusted_promotion: None,
    }
}

/// Advisory, verdict-independent data-quality flags and their human-readable
/// counterpart, computed together from the same rates/counts so the two
/// representations can't drift out of sync with each other. Kept separate
/// from `MetricOutput.warning` (which forces `Inconclusive` on zero usable
/// trials, a real verdict-changing decision) - these never affect `verdict`.
fn collect_data_quality(
    metric: MetricKind,
    out: &metrics::MetricOutput,
    paired_by_id: bool,
) -> (report::DataQuality, Vec<String>) {
    let mut quality = report::DataQuality::default();
    let mut warnings = Vec::new();

    quality.tiny_sample = out.paired_count < 30;
    if quality.tiny_sample {
        warnings.push(format!(
            "small sample: {} paired trial(s), below the conventional 30-trial threshold for confidence-interval methods to be reliable",
            out.paired_count
        ));
    }

    // ponytail: this treats `paired_count` and the failure counts as disjoint - true under
    // `FailurePolicy::ReportOnly`/`Exclude` for the common status-only-record case, but a record
    // carrying both a failure status and a counted outcome (a mixed status+result record under
    // `ReportOnly`, or *any* failure under `Loss`, whose synthesized outcome lands in
    // `paired_count` too) is double-counted here: once as a failure, once as a trial. This can
    // under-report `high_failure_rate` for a true failure rate a little above 20% (e.g. true 22%
    // reports as ~18%, a real miss). Advisory-only - never affects `verdict` - and the practical
    // miss window is narrow (only near the 20% boundary; far above or below it the discount
    // doesn't change which side of 20% it lands on). Fix properly if this bites in practice: track
    // "outcome came from a failure" separately per aggregator and exclude it from `paired_count`
    // here, rather than trying to disentangle it from this already-summed total.
    let usable_trials = if metric == MetricKind::PairedElo {
        out.paired_count.saturating_mul(2)
    } else {
        out.paired_count
    };
    let total_trials = usable_trials + out.timeouts + out.crashes + out.invalid;
    if total_trials > 0 {
        let failure_rate = (out.timeouts + out.crashes + out.invalid) as f64 / total_trials as f64;
        quality.high_failure_rate = failure_rate > 0.2;
        if quality.high_failure_rate {
            warnings.push(format!(
                "{:.0}% of trials failed to execute (timeout/crash/invalid) rather than producing a usable result",
                failure_rate * 100.0
            ));
        }
    }

    // winrate/sign-test discard their tie/draw count before it reaches
    // MetricOutput, so extending this warning to them would need a new
    // tracked field - deferred, not silently dropped.
    if metric == MetricKind::Elo && out.paired_count > 0 {
        let draws = out
            .paired_count
            .saturating_sub(out.baseline_count + out.candidate_count);
        let draw_rate = draws as f64 / out.paired_count as f64;
        quality.draw_heavy = draw_rate > 0.5;
        if quality.draw_heavy {
            warnings.push(format!(
                "{:.0}% of trials were draws, leaving few decisive outcomes to estimate Elo from",
                draw_rate * 100.0
            ));
        }
    }

    // Deliberately guarded by !tiny_sample - see DataQuality's doc comment
    // for why a wide CI from a tiny sample shouldn't also trip this.
    quality.effect_within_noise_floor =
        !quality.tiny_sample && out.effect.abs() < (out.ci_high - out.ci_low) / 2.0;
    if quality.effect_within_noise_floor {
        warnings.push(
            "the measured effect is smaller than the CI's own half-width: it could plausibly be noise around zero, even though the sample isn't tiny"
                .to_string(),
        );
    }

    // `quantile-diff` only. Distribution-free proxy for "is there enough data in the thinner
    // tail to estimate this quantile at all" - same shape as the binomial `np >= 10` rule of
    // thumb, using the expected count in whichever tail is smaller. Deliberately NOT guarded by
    // `!tiny_sample`: unlike `effect_within_noise_floor` (a redundant restatement of "the sample
    // is small" once n alone already flags it), this carries information `tiny_sample`'s n-only
    // threshold can't see - e.g. n=100 at q=0.95 has only 5 expected tail observations and should
    // fire even though `paired_count < 30` is false.
    if let Some(q) = out.quantile
        && out.paired_count > 0
    {
        quality.thin_quantile_tail = out.paired_count as f64 * q.min(1.0 - q) < 10.0;
        if quality.thin_quantile_tail {
            warnings.push(format!(
                "thin quantile tail: only ~{:.1} expected observation(s) in the thinner tail at q={:.2} with {} paired trial(s) - this quantile estimate is likely unreliable",
                out.paired_count as f64 * q.min(1.0 - q),
                q,
                out.paired_count
            ));
        }
    }

    // `mean-diff` only - `relative-diff` ships `scale_diagnostics` too (for transparency/
    // machine-readability) but never sets `wide_baseline_scale` on itself: it's already the
    // proportional-change metric, so it has nothing to warn its own user to switch away from.
    // Computed from `out.scale_diagnostics` alone, which was itself built from baseline values
    // only (see `ScaleDiagnostics`'s doc) - this never inspects `out.effect`/`out.ci_low`/
    // `out.ci_high`, so the warning can't be, even accidentally, a function of the observed result.
    if metric == MetricKind::MeanDiff
        && let Some(diag) = &out.scale_diagnostics
    {
        // Robust (p95/p05) span is the primary signal once there's enough data for a single
        // outlier not to define it alone; below that floor, the raw min/max span is all there is.
        let effective_orders =
            if diag.positive_baseline_count >= metrics::ROBUST_SPAN_MIN_POSITIVE_BASELINES as u64 {
                diag.robust_orders_of_magnitude
                    .unwrap_or(diag.raw_orders_of_magnitude)
            } else {
                diag.raw_orders_of_magnitude
            };
        quality.wide_baseline_scale = effective_orders >= report::WIDE_BASELINE_SCALE_ORDERS;
        if quality.wide_baseline_scale {
            if diag.non_positive_baseline_count == 0 {
                warnings.push(format!(
                    "baseline values span {effective_orders:.1} orders of magnitude; absolute \
                     differences may be dominated by larger-scale cases. If the scientific \
                     question is proportional change, consider --metric relative-diff. Choose the \
                     metric before confirmatory analysis; switching after inspecting the verdict \
                     is exploratory."
                ));
            } else {
                warnings.push(
                    "baseline values vary widely, but some baselines are zero or negative, so \
                     relative-diff is not well-defined for this dataset. Use a domain-justified \
                     normalization rather than adding an arbitrary denominator offset."
                        .to_string(),
                );
            }
        }
    }

    // `relative-diff` only, and silent entirely under `--paired-by-id` (same convention as
    // `low_id_diversity` below - repeated ids mean something different there). `out.tied_count` is
    // counted at ingest (pre-netting - see `RelativeDiffAggregator::ingest`), while
    // `out.baseline_count` is post-netting; under `--paired-by-id` this isn't a small
    // approximation but a real, unbounded skew - two records from the same tied pair both count
    // toward `tied_count` but net to a single post-netting record, so the naive ratio can run past
    // 100% (verified: two exact-match pairs plus a distinct netted-to-zero pair produces
    // `tied_count == 2`, `baseline_count == 2`, i.e. a 2x-inflated 100% reading against the true
    // 50% pre-netting rate). Reporting a real pre-netting denominator would need new plumbing this
    // round doesn't add without a concrete request for it - see docs/research-map.md's
    // "subset-only relative-diff" entry, which already covers the closely related question of a
    // pre-netting-aware subset effect size.
    if !paired_by_id
        && let Some(tied_count) = out.tied_count
        && out.baseline_count > 0
    {
        let tied_fraction = tied_count as f64 / out.baseline_count as f64;
        quality.diluted_by_ties = tied_fraction >= report::TIE_DILUTION_FRACTION;
        if quality.diluted_by_ties {
            warnings.push(format!(
                "more than half of paired records show no change between candidate and baseline \
                 ({tied_count} of {}); if only a subset of cases was actually affected by this \
                 change, the pooled relative-diff effect is diluted toward zero by the unaffected \
                 majority - see docs/research-map.md's \"subset-only relative-diff\" entry for a \
                 deferred idea to report a target-subset effect size separately",
                out.baseline_count
            ));
        }
    }

    // records_with_id/max_id_count are 0 when --paired-by-id is set - paired mode already has its
    // own meaning for a repeated id, so the shared advisory remains silent there.
    let (low_id_diversity, id_warning) =
        report::id_diversity_advisory(out.records_with_id, out.max_id_count);
    quality.low_id_diversity = low_id_diversity;
    if let Some(warning) = id_warning {
        warnings.push(warning);
    }

    (quality, warnings)
}
