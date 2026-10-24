//! Single-pass metric driver and aggregator construction.

use super::*;

/// One metric's independent, incremental computation. `ingest` is called once per record
/// (`baseline_status`/`candidate_status` are parsed once per record by the shared driver below,
/// since status-tally logic is identical regardless of which metric is running - today that
/// work would otherwise be redone per metric). Passed as parsed `TrialStatus`, not a resolved
/// `Outcome`: `mean-diff`/`sign-test` have no use for an outcome (they only ever check
/// presence, via `.is_some()`), while `winrate`/`elo` resolve it themselves via
/// `effective_outcome` using their own stored `FailurePolicy` - a shared "resolved outcome"
/// wouldn't fit the numeric metrics at all. If this aggregator finds none of its own required
/// fields on a record AND both statuses are `None`, it must reject with `SchemaMismatch` - this
/// preserves strict per-metric validation even when several metrics share one pass: a record
/// usable by metric A but not metric B still errors when B is requested, exactly as if B had
/// run alone.
pub(crate) trait MetricAggregator {
    fn ingest(
        &mut self,
        line: usize,
        record: &Record,
        baseline_status: Option<TrialStatus>,
        candidate_status: Option<TrialStatus>,
    ) -> Result<(), VeridictError>;
    fn finish(self: Box<Self>, failures: &FailureBreakdown) -> Result<MetricOutput, VeridictError>;
}

#[allow(clippy::too_many_arguments)]
fn build_aggregator(
    config: MetricConfig,
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Box<dyn MetricAggregator> {
    match config {
        MetricConfig::WinRate {
            ci_method,
            failure_policy,
        } => Box::new(winrate::WinRateAggregator::new(
            confidence,
            paired_by_id,
            cluster_by_id,
            ci_method,
            failure_policy,
            resamples,
            seed,
        )),
        MetricConfig::Elo { failure_policy } => Box::new(elo::EloAggregator::new(
            confidence,
            paired_by_id,
            cluster_by_id,
            failure_policy,
            resamples,
            seed,
        )),
        MetricConfig::PairedElo { failure_policy } => Box::new(
            paired_elo::PairedEloAggregator::new(confidence, failure_policy, resamples, seed),
        ),
        MetricConfig::MeanDiff { bootstrap_method } => {
            Box::new(mean_diff::MeanDiffAggregator::new(
                confidence,
                resamples,
                seed,
                paired_by_id,
                bootstrap_method,
            ))
        }
        MetricConfig::SignTest { ci_method } => Box::new(sign_test::SignTestAggregator::new(
            confidence,
            paired_by_id,
            ci_method,
        )),
        MetricConfig::QuantileDiff {
            quantile,
            bootstrap_method,
        } => Box::new(quantile_diff::QuantileDiffAggregator::new(
            confidence,
            resamples,
            seed,
            paired_by_id,
            quantile,
            bootstrap_method,
        )),
        MetricConfig::RelativeDiff { bootstrap_method } => {
            Box::new(relative_diff::RelativeDiffAggregator::new(
                confidence,
                resamples,
                seed,
                paired_by_id,
                bootstrap_method,
            ))
        }
    }
}

/// Runs several metrics against the same records in a single pass: every
/// record is fed to every requested metric's aggregator, instead of
/// re-scanning `records` once per metric. `records` is consumed as a
/// streaming iterator, not a slice - `main.rs::read_records` hands this a
/// lazy iterator straight from the input file/stdin, so callers that only
/// need `winrate`/`elo`/`sign-test` (never `mean-diff`) see genuinely
/// bounded memory regardless of input size (`--paired-by-id` buffers
/// unresolved ids, which scales with distinct in-flight ids, not total
/// record count - see each aggregator's module doc). `mean-diff`'s
/// bootstrap always needs the full sample materialized internally
/// (`DiffCollector`'s `Vec<f64>`); that floor is inherent to random-access
/// resampling and out of scope to remove. `paired_by_id`: see each
/// aggregator's module doc - two records sharing the same `id` are combined
/// into one net observation instead of two independent ones. Each
/// `MetricConfig::new` validates CLI-shaped construction, while this entry
/// point revalidates the remaining public-enum invariants so direct library
/// construction cannot bypass quantile validation.
/// `records_with_id`/`max_id_count` (see `MetricOutput`'s doc) are tracked
/// here, centrally, rather than per-aggregator - it's the same fact
/// regardless of which metrics are requested, and per-aggregator tracking is
/// exactly how `OutcomeCollector` (no duplicate-id check) and
/// `DiffCollector`/`SignCounts` (hard error on any duplicate) ended up with
/// different behavior in the first place.
pub fn compute_many<I>(
    records: I,
    metrics: &[MetricConfig],
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<Vec<MetricOutput>, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    if metrics.is_empty() {
        return Err(VeridictError::InvalidThreshold(
            "at least one metric is required".to_string(),
        ));
    }
    for config in metrics {
        config.validate()?;
    }
    if metrics
        .iter()
        .any(|config| config.kind() == MetricKind::PairedElo)
        && !paired_by_id
    {
        return Err(VeridictError::PairedEloRequiresPairedById);
    }
    if cluster_by_id {
        if paired_by_id {
            return Err(VeridictError::ClusterByIdConflictsWithPairedById);
        }
        for &config in metrics {
            let kind = config.kind();
            if !matches!(kind, MetricKind::WinRate | MetricKind::Elo) {
                return Err(VeridictError::IncompatibleClusterById {
                    metric: metric_label(kind),
                });
            }
        }
    }

    let mut records = records
        .into_iter()
        .map(IntoRecordResult::into_record_result)
        .peekable();
    if records.peek().is_none() {
        return Err(VeridictError::EmptyInput);
    }
    if !confidence.is_finite() || confidence <= 0.0 || confidence >= 1.0 {
        return Err(VeridictError::InvalidConfidence(confidence));
    }
    let needs_resamples = cluster_by_id
        || metrics.iter().any(|config| {
            matches!(
                config.kind(),
                MetricKind::MeanDiff
                    | MetricKind::PairedElo
                    | MetricKind::QuantileDiff
                    | MetricKind::RelativeDiff
            )
        });
    if needs_resamples && resamples == 0 {
        return Err(VeridictError::InvalidResamples(resamples));
    }

    let mut failures = FailureBreakdown::default();
    let mut aggregators: Vec<Box<dyn MetricAggregator>> = metrics
        .iter()
        .map(|&config| {
            build_aggregator(
                config,
                confidence,
                resamples,
                seed,
                paired_by_id,
                cluster_by_id,
            )
        })
        .collect();

    // Paired/cluster mode both already have their own meaning for a repeated id (paired: nets to
    // one observation, or a hard `SchemaMismatch` past 2; cluster: the id IS the resampling unit)
    // - this tracking is only about the unpaired, unclustered gap, so it's skipped entirely under
    // either.
    let mut id_counts: HashMap<String, u64> = HashMap::new();
    let mut records_with_id: u64 = 0;

    for item in records {
        let (line, record) = item?;
        let mut baseline_status = None;
        let mut candidate_status = None;
        if let Some(status) = record.baseline_status.as_deref() {
            baseline_status = Some(tally_status(
                status,
                line,
                "baseline_status",
                &mut failures.baseline,
            )?);
        }
        if let Some(status) = record.candidate_status.as_deref() {
            candidate_status = Some(tally_status(
                status,
                line,
                "candidate_status",
                &mut failures.candidate,
            )?);
        }
        if !paired_by_id
            && !cluster_by_id
            && let Some(id) = record.id.as_deref()
        {
            records_with_id += 1;
            *id_counts.entry(id.to_string()).or_insert(0) += 1;
        }
        for agg in &mut aggregators {
            agg.ingest(line, &record, baseline_status, candidate_status)?;
        }
    }
    let max_id_count = id_counts.values().copied().max().unwrap_or(0);

    metrics
        .iter()
        .copied()
        .zip(aggregators)
        .map(|(config, agg)| {
            let mut out = agg.finish(&failures)?;
            validate_finite_output(config.kind(), &out)?;
            out.records_with_id = records_with_id;
            out.max_id_count = max_id_count;
            Ok(out)
        })
        .collect()
}
/// Single-metric convenience wrapper around [`compute_many`].
pub fn compute<I>(
    records: I,
    metric: MetricConfig,
    confidence: f64,
    resamples: usize,
    seed: u64,
    paired_by_id: bool,
    cluster_by_id: bool,
) -> Result<MetricOutput, VeridictError>
where
    I: IntoIterator,
    I::Item: IntoRecordResult,
{
    compute_many(
        records,
        std::slice::from_ref(&metric),
        confidence,
        resamples,
        seed,
        paired_by_id,
        cluster_by_id,
    )
    .map(|mut outs| outs.remove(0))
}
