//! Incremental state, checkpointing, validation, and report finalization.

use super::*;

/// Version of the serialized [`SprtCheckpoint`] shape. This versions resumable state, not the
/// final [`SprtReport`] JSON contract.
pub const SPRT_CHECKPOINT_VERSION: u32 = 1;

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Position and deterministic prefix identity of an incremental SPRT stream. Save this alongside
/// the producer's own durable cursor and pass it back to [`SprtAccumulator::from_checkpoint`].
/// The hash detects accidental prefix/config mix-ups; it is not a cryptographic authenticity tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SprtStreamPosition {
    pub record_count: u64,
    pub last_line: Option<usize>,
    pub prefix_hash: u64,
}

impl Default for SprtStreamPosition {
    fn default() -> Self {
        Self {
            record_count: 0,
            last_line: None,
            prefix_hash: FNV_OFFSET_BASIS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SprtAccumulationState {
    failures: FailureBreakdown,
    collector: SequentialOutcomeCollector,
    unpaired: UnpairedOutcomeAccumulator,
    pentanomial_collector: PentanomialCollector,
    records_with_id: u64,
    id_counts: HashMap<String, u64>,
}

impl SprtAccumulationState {
    fn new(options: SprtRunOptions) -> Self {
        Self {
            failures: FailureBreakdown::default(),
            collector: SequentialOutcomeCollector::new(
                options.paired_by_id,
                options.require_complete_pairs,
            ),
            unpaired: UnpairedOutcomeAccumulator::default(),
            pentanomial_collector: PentanomialCollector::new(),
            records_with_id: 0,
            id_counts: HashMap::new(),
        }
    }

    fn push(
        &mut self,
        line: usize,
        record: &Record,
        config: &SprtConfig,
        options: SprtRunOptions,
    ) -> Result<(), VeridictError> {
        // Parse into a local tally first. A malformed record must not partially mutate a resumable
        // checkpoint before returning its error.
        let mut failures = self.failures;
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
        let used =
            baseline_status.is_some() || candidate_status.is_some() || record.result.is_some();
        let outcome = effective_outcome(
            options.failure_policy,
            baseline_status,
            candidate_status,
            record.result.as_deref(),
            line,
        )?;
        if !used {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: "sprt",
                detail: "record has no result and no status fields".to_string(),
            });
        }
        if options.variant == SprtVariant::Pentanomial && outcome.is_some() && record.id.is_none() {
            return Err(VeridictError::SchemaMismatch {
                line,
                context: "pentanomial",
                detail: "record has no id; --sprt-variant pentanomial requires every record to \
                         carry one"
                    .to_string(),
            });
        }

        if let Some(outcome) = outcome {
            if options.variant == SprtVariant::Pentanomial {
                self.pentanomial_collector.record(
                    line,
                    record.id.as_deref().expect("pentanomial id checked above"),
                    outcome,
                );
            } else if options.paired_by_id {
                self.collector.record(line, record.id.as_deref(), outcome);
            } else {
                let bounds = math::bounds(config.alpha, config.beta);
                let mut unpaired = self.unpaired;
                unpaired.record(outcome, config, options.variant, &bounds)?;
                self.unpaired = unpaired;
            }
        }
        if !options.paired_by_id
            && let Some(id) = record.id.as_deref()
        {
            self.records_with_id += 1;
            *self.id_counts.entry(id.to_string()).or_insert(0) += 1;
        }
        self.failures = failures;
        Ok(())
    }
}

/// Serializable state for resuming an incremental SPRT. Fields are private so callers cannot
/// construct a partly-valid state; use [`SprtAccumulator::snapshot`] and serde.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SprtCheckpoint {
    pub(super) checkpoint_version: u32,
    config: SprtConfig,
    options: SprtRunOptions,
    position: SprtStreamPosition,
    state: SprtAccumulationState,
}

impl SprtCheckpoint {
    pub fn position(&self) -> SprtStreamPosition {
        self.position
    }
}

/// Incremental, resumable counterpart to [`run_with_options`]. Records must be pushed in stream
/// order. `snapshot()` contains the first-crossing state, failure tallies, incomplete pair state,
/// configuration identity, and prefix fingerprint needed for a crash-safe resume.
#[derive(Debug)]
pub struct SprtAccumulator {
    checkpoint: SprtCheckpoint,
}

impl SprtAccumulator {
    /// Starts an empty incremental run after validating the hypotheses and execution options.
    pub fn new(config: SprtConfig, options: SprtRunOptions) -> Result<Self, VeridictError> {
        validate_sprt_run(&config, options)?;
        Ok(Self {
            checkpoint: SprtCheckpoint {
                checkpoint_version: SPRT_CHECKPOINT_VERSION,
                config,
                options,
                position: SprtStreamPosition::default(),
                state: SprtAccumulationState::new(options),
            },
        })
    }

    /// Returns the durable stream position to persist alongside the producer's own cursor.
    pub fn position(&self) -> SprtStreamPosition {
        self.checkpoint.position
    }

    /// Adds the next record. `line` must be strictly greater than the preceding pushed line.
    pub fn push(&mut self, line: usize, record: Record) -> Result<(), VeridictError> {
        self.push_record(line, record, true)
    }

    pub(super) fn push_batch(&mut self, line: usize, record: Record) -> Result<(), VeridictError> {
        self.push_record(line, record, false)
    }

    fn push_record(
        &mut self,
        line: usize,
        record: Record,
        require_monotonic_line: bool,
    ) -> Result<(), VeridictError> {
        if let Some(previous) = self.checkpoint.position.last_line
            && require_monotonic_line
            && line <= previous
        {
            return Err(VeridictError::NonMonotonicSprtLine { line, previous });
        }
        self.checkpoint.state.push(
            line,
            &record,
            &self.checkpoint.config,
            self.checkpoint.options,
        )?;
        self.checkpoint.position.prefix_hash =
            hash_record_prefix(self.checkpoint.position.prefix_hash, line, &record);
        self.checkpoint.position.record_count += 1;
        self.checkpoint.position.last_line = Some(line);
        Ok(())
    }

    /// Clones the complete serializable state without changing the running test.
    pub fn snapshot(&self) -> SprtCheckpoint {
        self.checkpoint.clone()
    }

    /// Restores a checkpoint only when its version, hypotheses, execution policies, and durable
    /// producer cursor all match. `expected_position` should come from the producer's own saved
    /// run metadata rather than from the checkpoint being restored.
    pub fn from_checkpoint(
        checkpoint: SprtCheckpoint,
        config: SprtConfig,
        options: SprtRunOptions,
        expected_position: SprtStreamPosition,
    ) -> Result<Self, VeridictError> {
        if checkpoint.checkpoint_version != SPRT_CHECKPOINT_VERSION {
            return Err(VeridictError::UnsupportedSprtCheckpointVersion {
                found: checkpoint.checkpoint_version,
                supported: SPRT_CHECKPOINT_VERSION,
            });
        }
        validate_sprt_run(&config, options)?;
        if checkpoint.config != config {
            return Err(VeridictError::SprtCheckpointMismatch {
                detail: "hypotheses or alpha/beta differ".to_string(),
            });
        }
        if checkpoint.options != options {
            return Err(VeridictError::SprtCheckpointMismatch {
                detail: "SPRT variant or execution options differ".to_string(),
            });
        }
        if checkpoint.position != expected_position {
            return Err(VeridictError::SprtCheckpointMismatch {
                detail: "stream record count, last line, or prefix fingerprint differs".to_string(),
            });
        }
        Ok(Self { checkpoint })
    }

    /// Returns a report for the prefix accumulated so far without consuming the accumulator.
    /// The same final validation as the batch API applies, including incomplete-pair errors.
    pub fn report(&self) -> Result<SprtReport, VeridictError> {
        finalize_sprt(self.checkpoint.clone())
    }

    /// Consumes the accumulator and performs the same final validation as the batch API.
    pub fn finish(self) -> Result<SprtReport, VeridictError> {
        finalize_sprt(self.checkpoint)
    }
}

fn hash_record_prefix(mut hash: u64, line: usize, record: &Record) -> u64 {
    fn bytes(hash: &mut u64, data: &[u8]) {
        for byte in data {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    fn option_str(hash: &mut u64, value: Option<&str>) {
        match value {
            Some(value) => {
                bytes(hash, &[1]);
                bytes(hash, &(value.len() as u64).to_le_bytes());
                bytes(hash, value.as_bytes());
            }
            None => bytes(hash, &[0]),
        }
    }
    fn option_f64(hash: &mut u64, value: Option<f64>) {
        match value {
            Some(value) => {
                bytes(hash, &[1]);
                bytes(hash, &value.to_bits().to_le_bytes());
            }
            None => bytes(hash, &[0]),
        }
    }

    bytes(&mut hash, &(line as u64).to_le_bytes());
    option_str(&mut hash, record.id.as_deref());
    option_f64(&mut hash, record.baseline);
    option_f64(&mut hash, record.candidate);
    option_str(&mut hash, record.result.as_deref());
    option_str(&mut hash, record.baseline_status.as_deref());
    option_str(&mut hash, record.candidate_status.as_deref());
    hash
}

fn validate_sprt_run(config: &SprtConfig, options: SprtRunOptions) -> Result<(), VeridictError> {
    SprtConfig::new(config.elo0, config.elo1, config.alpha, config.beta)?;
    if options.variant == SprtVariant::Pentanomial && !options.paired_by_id {
        return Err(VeridictError::InvalidThreshold(
            "--sprt-variant pentanomial requires --paired-by-id".to_string(),
        ));
    }
    if options.variant != SprtVariant::Pentanomial
        && (options.min_paired_ids.is_some() || options.max_paired_ids.is_some())
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids/--max-paired-ids require --sprt-variant pentanomial (the only \
             variant with a paired_count)"
                .to_string(),
        ));
    }
    if let Some(min) = options.min_paired_ids
        && min < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--min-paired-ids must be >= 1".to_string(),
        ));
    }
    if let Some(max) = options.max_paired_ids
        && max < 1
    {
        return Err(VeridictError::InvalidThreshold(
            "--max-paired-ids must be >= 1".to_string(),
        ));
    }
    if let (Some(min), Some(max)) = (options.min_paired_ids, options.max_paired_ids)
        && min > max
    {
        return Err(VeridictError::InvalidThreshold(format!(
            "--min-paired-ids ({min}) must be <= --max-paired-ids ({max})"
        )));
    }
    Ok(())
}

pub(super) fn require_finite_sprt(field: &'static str, value: f64) -> Result<(), VeridictError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(VeridictError::NonFiniteStatistic {
            metric: "sprt",
            field,
            value,
        })
    }
}

pub(super) fn outcome_llr(
    variant: SprtVariant,
    config: &SprtConfig,
    candidate_wins: u64,
    baseline_wins: u64,
    draws: u64,
) -> Result<(f64, Option<f64>), VeridictError> {
    let result = match variant {
        SprtVariant::Wald => {
            let p0 = math::score_from_elo(config.elo0);
            let p1 = math::score_from_elo(config.elo1);
            (
                candidate_wins as f64 * math::llr_delta(true, p0, p1)
                    + baseline_wins as f64 * math::llr_delta(false, p0, p1),
                None,
            )
        }
        SprtVariant::Trinomial => {
            if candidate_wins + baseline_wins + draws == 0 {
                (0.0, Some(0.0))
            } else {
                let (llr, drawelo) = trinomial_sprt::llr(
                    config.elo0,
                    config.elo1,
                    candidate_wins,
                    draws,
                    baseline_wins,
                );
                (llr, Some(drawelo))
            }
        }
        SprtVariant::Pentanomial => unreachable!("pentanomial has its own sequential walk"),
    };
    require_finite_sprt("llr", result.0)?;
    if let Some(drawelo) = result.1 {
        require_finite_sprt("drawelo", drawelo)?;
    }
    Ok(result)
}

fn evaluate_outcomes_sequential(
    observations: &[OrderedOutcome],
    config: &SprtConfig,
    variant: SprtVariant,
    bounds: &math::SprtBounds,
) -> Result<OutcomeWalk, VeridictError> {
    let available_observation_count = observations.len() as u64;
    let available_record_count = observations
        .iter()
        .map(|observation| observation.raw_record_count)
        .sum();
    let (mut candidate_wins, mut baseline_wins, mut draws) = (0, 0, 0);
    let mut analyzed_record_count = 0;

    for (index, observation) in observations.iter().enumerate() {
        tally_outcome(
            observation.outcome,
            &mut candidate_wins,
            &mut baseline_wins,
            &mut draws,
        );
        analyzed_record_count += observation.raw_record_count;
        let analyzed_observation_count = (index + 1) as u64;
        let (llr, drawelo) = outcome_llr(variant, config, candidate_wins, baseline_wins, draws)?;
        let stopping_reason = if llr >= bounds.upper {
            Some("upper_bound_crossed")
        } else if llr <= bounds.lower {
            Some("lower_bound_crossed")
        } else {
            None
        };
        if let Some(stopping_reason) = stopping_reason {
            return Ok(OutcomeWalk {
                llr,
                drawelo,
                candidate_wins,
                baseline_wins,
                draws,
                available_observation_count,
                analyzed_observation_count,
                stopping_observation_count: Some(analyzed_observation_count),
                stopping_reason,
                available_record_count,
                analyzed_record_count,
            });
        }
    }

    let (llr, drawelo) = outcome_llr(variant, config, candidate_wins, baseline_wins, draws)?;
    Ok(OutcomeWalk {
        llr,
        drawelo,
        candidate_wins,
        baseline_wins,
        draws,
        available_observation_count,
        analyzed_observation_count: available_observation_count,
        stopping_observation_count: None,
        stopping_reason: "insufficient_data",
        available_record_count,
        analyzed_record_count: available_record_count,
    })
}

fn finalize_sprt(checkpoint: SprtCheckpoint) -> Result<SprtReport, VeridictError> {
    if checkpoint.position.record_count == 0 {
        return Err(VeridictError::EmptyInput);
    }
    validate_sprt_run(&checkpoint.config, checkpoint.options)?;
    let config = checkpoint.config;
    let SprtRunOptions {
        variant,
        paired_by_id,
        failure_policy: _,
        require_complete_pairs,
        min_paired_ids,
        max_paired_ids,
    } = checkpoint.options;
    let SprtAccumulationState {
        failures,
        collector,
        unpaired,
        pentanomial_collector,
        records_with_id,
        id_counts,
    } = checkpoint.state;
    let bounds = math::bounds(config.alpha, config.beta);

    let timeouts = failures.baseline.timeout + failures.candidate.timeout;
    let crashes = failures.baseline.crash + failures.candidate.crash;
    let invalid = failures.baseline.invalid + failures.candidate.invalid;
    let max_id_count = id_counts.values().copied().max().unwrap_or(0);
    let (low_id_diversity, id_warning) =
        crate::report::id_diversity_advisory(records_with_id, max_id_count);
    let warnings = id_warning.into_iter().collect();
    let data_quality = SprtDataQuality { low_id_diversity };

    let analysis = match variant {
        SprtVariant::Wald | SprtVariant::Trinomial => {
            if !paired_by_id {
                unpaired.finish(&config, variant)?
            } else {
                let observations = collector.finish()?;
                let (mut candidate_wins, mut baseline_wins, mut draws) = (0, 0, 0);
                for observation in &observations {
                    tally_outcome(
                        observation.outcome,
                        &mut candidate_wins,
                        &mut baseline_wins,
                        &mut draws,
                    );
                }
                let (llr, drawelo) =
                    outcome_llr(variant, &config, candidate_wins, baseline_wins, draws)?;
                let walk = evaluate_outcomes_sequential(&observations, &config, variant, &bounds)?;
                RunAnalysis {
                    llr,
                    drawelo,
                    candidate_wins,
                    baseline_wins,
                    draws,
                    decision_llr: walk.llr,
                    decision_drawelo: walk.drawelo,
                    decision_candidate_wins: walk.candidate_wins,
                    decision_baseline_wins: walk.baseline_wins,
                    decision_draws: walk.draws,
                    analysis_unit: if paired_by_id { "pair" } else { "trial" },
                    available_observation_count: walk.available_observation_count,
                    analyzed_observation_count: walk.analyzed_observation_count,
                    stopping_observation_count: walk.stopping_observation_count,
                    stopping_observation_reason: walk.stopping_reason,
                    available_record_count: walk.available_record_count,
                    analyzed_record_count: walk.analyzed_record_count,
                    pentanomial: None,
                }
            }
        }
        SprtVariant::Pentanomial => {
            let (completed_in_order, raw_trial_count) = pentanomial_collector.finish()?;
            let walk = evaluate_pentanomial_sequential(
                &completed_in_order,
                &config,
                &bounds,
                min_paired_ids,
                max_paired_ids,
            )?;
            let llr = walk.llr;
            let (baseline_wins, candidate_wins, draws) = net_pentanomial_buckets(&walk.buckets);
            RunAnalysis {
                llr,
                drawelo: None,
                candidate_wins,
                baseline_wins,
                draws,
                decision_llr: llr,
                decision_drawelo: None,
                decision_candidate_wins: candidate_wins,
                decision_baseline_wins: baseline_wins,
                decision_draws: draws,
                analysis_unit: "pair",
                available_observation_count: walk.available_paired_count,
                analyzed_observation_count: walk.analyzed_paired_count,
                stopping_observation_count: walk.stopping_pair_count,
                stopping_observation_reason: walk.stopping_reason,
                available_record_count: raw_trial_count,
                analyzed_record_count: walk.analyzed_paired_count * 2,
                pentanomial: Some((walk, raw_trial_count)),
            }
        }
    };

    let unit = if variant == SprtVariant::Trinomial {
        "belo"
    } else {
        "elo"
    };
    let (verdict, mut reason) = match analysis.stopping_observation_reason {
        "upper_bound_crossed" => (
            Verdict::Pass,
            format!(
                "LLR {:.3} reached the upper bound {:.3}: reject H0 ({unit} <= {:+.1}), accept H1 ({unit} >= {:+.1})",
                analysis.decision_llr, bounds.upper, config.elo0, config.elo1
            ),
        ),
        "lower_bound_crossed" => (
            Verdict::Fail,
            format!(
                "LLR {:.3} reached the lower bound {:.3}: reject H1 ({unit} >= {:+.1}), accept H0 ({unit} <= {:+.1})",
                analysis.decision_llr, bounds.lower, config.elo1, config.elo0
            ),
        ),
        "max_paired_ids_reached" | "insufficient_data" => (
            Verdict::Inconclusive,
            format!(
                "LLR {:.3} is within ({:.3}, {:.3}): keep testing",
                analysis.decision_llr, bounds.lower, bounds.upper
            ),
        ),
        _ => unreachable!("sequential walk returned an unknown stopping reason"),
    };

    let (
        pentanomial_counts,
        raw_trial_count,
        paired_count,
        available_paired_count,
        stopping_pair_count,
        stopping_reason,
        ignored_pairs_after_stop,
    ) = match analysis.pentanomial {
        Some((walk, raw_trial_count)) => {
            if walk.stopping_reason == "insufficient_data"
                && min_paired_ids.is_some_and(|min| walk.analyzed_paired_count < min)
            {
                reason = format!(
                    "Only {} of minimum {} paired ids completed; strength not evaluated yet.",
                    walk.analyzed_paired_count,
                    min_paired_ids.unwrap()
                );
            } else if walk.stopping_reason == "max_paired_ids_reached" {
                reason = format!(
                    "{reason} (reached --max-paired-ids {} without crossing a boundary)",
                    max_paired_ids.unwrap()
                );
            }
            (
                Some(PentanomialCounts::from_buckets(walk.buckets)),
                Some(raw_trial_count),
                Some(walk.analyzed_paired_count),
                Some(walk.available_paired_count),
                walk.stopping_pair_count,
                Some(walk.stopping_reason),
                Some(walk.available_paired_count - walk.analyzed_paired_count),
            )
        }
        None => (None, None, None, None, None, None, None),
    };

    Ok(SprtReport {
        schema_version: SPRT_REPORT_SCHEMA_VERSION,
        verdict,
        validity: Validity::Valid,
        promotion: Promotion::decide(Validity::Valid, verdict),
        llr: analysis.llr,
        decision_llr: analysis.decision_llr,
        lower_bound: bounds.lower,
        upper_bound: bounds.upper,
        elo0: config.elo0,
        elo1: config.elo1,
        alpha: config.alpha,
        beta: config.beta,
        candidate_wins: analysis.candidate_wins,
        baseline_wins: analysis.baseline_wins,
        draws: analysis.draws,
        decision_candidate_wins: analysis.decision_candidate_wins,
        decision_baseline_wins: analysis.decision_baseline_wins,
        decision_draws: analysis.decision_draws,
        timeouts,
        crashes,
        invalid,
        failure_breakdown: failures,
        warnings,
        data_quality,
        drawelo: analysis.drawelo,
        decision_drawelo: analysis.decision_drawelo,
        sprt_variant: variant.label(),
        pentanomial_counts,
        raw_trial_count,
        paired_count,
        available_paired_count,
        stopping_pair_count,
        stopping_reason,
        ignored_pairs_after_stop,
        analysis_unit: analysis.analysis_unit,
        available_observation_count: analysis.available_observation_count,
        analyzed_observation_count: analysis.analyzed_observation_count,
        stopping_observation_count: analysis.stopping_observation_count,
        stopping_observation_reason: analysis.stopping_observation_reason,
        ignored_observations_after_stop: analysis.available_observation_count
            - analysis.analyzed_observation_count,
        available_record_count: analysis.available_record_count,
        analyzed_record_count: analysis.analyzed_record_count,
        ignored_records_after_stop: analysis.available_record_count
            - analysis.analyzed_record_count,
        min_paired_ids,
        max_paired_ids,
        require_complete_pairs,
        reason,
    })
}
