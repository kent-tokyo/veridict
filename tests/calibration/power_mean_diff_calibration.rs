//! Empirical verification that `power::estimate_trials`'s mean-diff closed form (a *normal
//! approximation* of the real bootstrap decision rule, see `src/power.rs`'s module doc) actually
//! predicts the real rule's pass rate: draw `estimated_trials`-many synthetic paired diffs from
//! `Normal(assume_effect, assume_sd)` (the exact model the formula assumes), run them through the
//! **real** `compare --metric mean-diff` bootstrap-CI decision (`veridict::compare_one`, not a
//! simulated approximation of it), and measure the empirical pass rate against `target_power`.
//!
//! Unlike the binomial `power_calibration.rs` (an *exact* search against an *exact* CI function),
//! this validates an approximation (normal model) against a rule with its own resampling noise
//! (the bootstrap) - a real, wider gap is expected here, not a bug. `#[ignore]`d for the same
//! reason `bootstrap_coverage.rs` is: each simulated experiment re-runs a real bootstrap CI, so
//! `SIMULATIONS`-many repeats cost far more than an O(1)-per-draw exact CI call. Run explicitly
//! with `cargo test --test calibration -- --ignored power_mean_diff_calibration` (or
//! `--include-ignored` to run everything).

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use veridict::input::Record;
use veridict::power::{PowerMetric, estimate_trials};
use veridict::verdict::Thresholds;
use veridict::{BootstrapMethod, MetricConfig, Verdict};

const RESAMPLES: usize = 500;
const SIMULATIONS: u64 = 500;
const SEED: u64 = 0x5EED;

/// Box-Muller transform (two independent `Uniform(0,1)` draws -> one standard normal sample) -
/// self-contained here rather than reusing `power.rs`'s own `inverse_normal_cdf` (`pub(crate)`,
/// unreachable from this external test crate). A standard textbook transform, fine to hand-write
/// for synthetic test-data generation - not a correctness-critical library path.
fn standard_normal(rng: &mut StdRng) -> f64 {
    let u1: f64 = rng.random_range(1e-12..1.0);
    let u2: f64 = rng.random::<f64>();
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

fn synthetic_diff_records(n: u64, mean: f64, sd: f64, rng: &mut StdRng) -> Vec<(usize, Record)> {
    (0..n)
        .map(|i| {
            let diff = mean + sd * standard_normal(rng);
            (
                i as usize,
                Record {
                    id: None,
                    baseline: Some(0.0),
                    candidate: Some(diff),
                    result: None,
                    baseline_status: None,
                    candidate_status: None,
                },
            )
        })
        .collect()
}

/// Empirical pass rate of an `n`-trial `compare --metric mean-diff` run drawn from
/// `Normal(mean, sd)` - the same quantity `power::estimate_trials_mean_diff`'s `achieved_power`
/// claims to predict via the normal-approximation formula, measured independently against the
/// real bootstrap rule instead of recomputed analytically.
fn empirical_pass_rate(n: u64, mean: f64, sd: f64, min_effect: f64, confidence: f64) -> f64 {
    let mut rng = StdRng::seed_from_u64(SEED);
    let thresholds = Thresholds::symmetric(min_effect).unwrap();
    let mut passed = 0u64;
    for sim in 0..SIMULATIONS {
        let records = synthetic_diff_records(n, mean, sd, &mut rng);
        let report = veridict::compare_one(
            records,
            MetricConfig::MeanDiff {
                bootstrap_method: BootstrapMethod::Percentile,
            },
            confidence,
            &thresholds,
            RESAMPLES,
            SEED.wrapping_add(sim),
            false,
            false,
        )
        .unwrap();
        if report.verdict == Verdict::Pass {
            passed += 1;
        }
    }
    passed as f64 / SIMULATIONS as f64
}

// Observed once at SIMULATIONS=500, RESAMPLES=500, SEED=0x5EED (recorded so a future reader can
// see the margin without re-running): min=0.02/assume=0.10/sd=0.15/target=0.80 -> n=28, empirical
// 0.8100 (formula's own achieved_power=0.8057, diff ~0.004); min=0.0/assume=0.15/sd=0.20/
// target=0.90 -> n=19, empirical 0.8900 (achieved_power=0.9048, diff ~0.015). Both track
// target_power closely - no systematic directional bias the way SPRT's ASN formula had one (that
// was a real, one-sided, ~1.6x effect; this is noise-scale in both directions). Tolerance is
// wider than the binomial calibration's (0.05) specifically because this validates an
// approximation against a rule with its own resampling noise on top - a real, expected gap, not a
// bug being masked.
#[test]
#[ignore]
fn mean_diff_achieved_power_tracks_target_power_empirically() {
    for (min_effect, assume_effect, assume_sd, target_power) in
        [(0.02, 0.10, 0.15, 0.80), (0.0, 0.15, 0.20, 0.90)]
    {
        let confidence = 0.95;
        let report = estimate_trials(
            PowerMetric::MeanDiff {
                assume_sd,
                sd_source: "assume-sd",
            },
            min_effect,
            assume_effect,
            confidence,
            target_power,
            false,
        )
        .unwrap();

        let empirical = empirical_pass_rate(
            report.estimated_trials,
            assume_effect,
            assume_sd,
            min_effect,
            confidence,
        );
        assert!(
            (empirical - target_power).abs() < 0.08,
            "mean-diff min={min_effect} assume={assume_effect} sd={assume_sd}: empirical pass \
             rate {empirical:.4} at n={} too far from target {target_power} (formula's own \
             achieved_power={:.4})",
            report.estimated_trials,
            report.achieved_power,
        );
    }
}
