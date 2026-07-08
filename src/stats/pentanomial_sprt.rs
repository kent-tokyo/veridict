//! Pentanomial (paired-game) generalized LLR SPRT: a draw-aware-and-then-some sequential
//! test over *pairs* of games (same opening, colors swapped) rather than individual games.
//!
//! This is a faithful port of Fishtest's `LLR_logistic` - the expectation-constrained
//! multinomial MLE ("exponential tilting" of the empirical pair-outcome distribution to a
//! hypothesized mean score), from `official-stockfish/fishtest`'s
//! `server/fishtest/stats/LLRcalc.py` (`LLR_logistic`/`MLE_expected`/`secular`). Units are
//! **logistic Elo** (`stats::sprt::score_from_elo`'s scale, the same as `SprtVariant::Wald`),
//! not `trinomial_sprt`'s BayesElo - this model has no `drawelo`-style nuisance parameter at
//! all, so there's nothing for a separate unit scale to represent.
//!
//! **Why this isn't just "trinomial on twice as many games":** a pentanomial pair's whole
//! statistical value comes from the *negative correlation* between its two games (the shared
//! opening's own bias helps one side in one game and hurts it in the other - that's what
//! `--paired-by-id` pairing is *for*). An earlier design for this module modeled a pair's
//! outcome as the convolution of two *independent* single-game draws; that sets the pair's
//! covariance to zero by construction and throws away the exact effect being measured, making
//! it strictly no better (and sometimes worse, since it also collapses "W+L" and "D+D" into
//! one indistinguishable bucket) than running `trinomial` directly on the ungrouped games. The
//! approach below avoids that: it never assumes a per-game model at all. It treats each pair's
//! outcome as one draw from a free 5-category multinomial, estimates that multinomial's shape
//! from the empirical pair-outcome frequencies directly (which already reflect whatever real
//! correlation exists in the data), and only constrains the *mean* pair score under each
//! hypothesis - so real pairing correlation, however strong, is preserved rather than modeled
//! away.
//!
//! Category `i` (0..=4) is pair score `i / 4.0` -> `{0.0, 0.25, 0.5, 0.75, 1.0}`, matching
//! Fishtest's own bucket definition (`LL`, `LD+DL`, `LW+DD+WL`, `DW+WD`, `WW` from the
//! candidate's perspective) - `pentanomial_llr`'s caller is responsible for mapping each pair's
//! two game outcomes to one of these 5 counts (see `sprt.rs`'s `PentanomialCollector`).

/// Mixed into any zero-count bucket before normalizing (matches `trinomial_sprt::P_EPSILON`'s
/// spirit, same magnitude Fishtest itself uses). **Load-bearing, not cosmetic**: a degenerate
/// all-mass-in-one-bucket sample (e.g. every pair scoring a clean 2-0) makes every other
/// bucket's empirical probability exactly zero, which makes the secular equation below
/// ill-posed (its bracket needs shifted values straddling zero on both sides, i.e. at least
/// one positive-probability bucket above and below the target mean) - regularizing keeps every
/// bucket at a tiny but nonzero share of the mass so the equation always has a valid bracket,
/// at the cost of a negligible bias for any realistic sample size.
const COUNT_EPSILON: f64 = 1e-3;

/// A discrete distribution over pair-outcome categories: `(score, probability)` per category.
type Pdf = Vec<(f64, f64)>;

fn regularize(counts: &[u64]) -> Vec<f64> {
    counts
        .iter()
        .map(|&c| if c == 0 { COUNT_EPSILON } else { c as f64 })
        .collect()
}

/// `(N, pdf)` from raw category counts: `N` is the regularized total (matches Fishtest's own
/// `results_to_pdf`, which sums the *regularized* counts, not the raw ones - the difference is
/// at most `COUNT_EPSILON` per empty bucket, negligible at any real sample size). Category `i`
/// of `len` categories has score `i / (len - 1)`.
fn results_to_pdf(counts: &[u64]) -> (f64, Pdf) {
    let regularized = regularize(counts);
    let n: f64 = regularized.iter().sum();
    let len = counts.len();
    let pdf = regularized
        .iter()
        .enumerate()
        .map(|(i, &r)| (i as f64 / (len - 1) as f64, r / n))
        .collect();
    (n, pdf)
}

/// Solve `sum_i p_i * a_i / (1 + x * a_i) = 0` for `x`, via bisection on the bracket
/// `(-1/max(a_i), -1/min(a_i))`.
///
/// The bracket is valid (finite endpoints, opposite-signed `f`) whenever `a_i` straddles zero,
/// which `regularize` guarantees (every category keeps a positive probability share, so both a
/// positive and a negative shifted value are always present as long as the target mean is
/// strictly between the smallest and largest category score - true for any finite Elo gap,
/// since `score_from_elo` only ever returns a value strictly inside `(0, 1)`).
///
/// Monotonicity justifies plain bisection instead of Fishtest's `scipy.optimize.brentq`: on
/// this bracket, `f'(x) = -sum_i p_i * a_i^2 / (1 + x*a_i)^2 <= 0` everywhere `f` is defined
/// (every term in that sum is non-negative), so `f` strictly decreases from `+inf` at the left
/// edge to `-inf` at the right edge with no other root in between - no bracketing subtlety a
/// fancier root-finder would help with.
fn secular(shifted: &[(f64, f64)]) -> f64 {
    let values = shifted.iter().map(|&(a, _)| a);
    let v = values.clone().fold(f64::INFINITY, f64::min);
    let w = values.fold(f64::NEG_INFINITY, f64::max);
    debug_assert!(
        v < 0.0 && w > 0.0,
        "secular equation requires shifted support straddling zero"
    );

    let f = |x: f64| -> f64 { shifted.iter().map(|&(a, p)| p * a / (1.0 + x * a)).sum() };

    let epsilon = 1e-9;
    let mut lo = -1.0 / w + epsilon;
    let mut hi = -1.0 / v - epsilon;
    // f(lo) > 0 and f(hi) < 0 by the monotonicity argument above; 100 bisection steps gives
    // ~2^-100 relative precision on this bounded bracket, far past f64's own precision floor.
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if f(mid) > 0.0 { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

/// Exponential-tilt (in the empirical-likelihood sense) `pdf` to have mean exactly `s`: the
/// maximum likelihood estimate of a discrete distribution's shape constrained to a given mean,
/// given an observed (empirical) distribution `pdf` - see Van den Bergh's
/// `support_MLE_multinomial.pdf`, Proposition 1.1, which this is a direct port of. Note this
/// never assumes a per-game generative model; it only reshapes `pdf`'s own empirical frequencies
/// to satisfy the mean constraint, which is exactly what lets it preserve whatever pair
/// correlation `pdf` already reflects (see module docs).
fn mle_expected(pdf: &Pdf, s: f64) -> Pdf {
    let shifted: Vec<(f64, f64)> = pdf.iter().map(|&(a, p)| (a - s, p)).collect();
    let x = secular(&shifted);
    pdf.iter()
        .map(|&(a, p)| (a, p / (1.0 + x * (a - s))))
        .collect()
}

/// Per-pair (i.e. divided by `N`) generalized log-likelihood ratio of `s1` versus `s0`, given
/// the empirical pair-outcome distribution `pdf`.
fn llr_per_pair(pdf: &Pdf, s0: f64, s1: f64) -> f64 {
    let pdf0 = mle_expected(pdf, s0);
    let pdf1 = mle_expected(pdf, s1);
    pdf.iter()
        .zip(pdf0.iter())
        .zip(pdf1.iter())
        .map(|((&(_, p), &(_, p0)), &(_, p1))| p * (p1.ln() - p0.ln()))
        .sum()
}

/// Total generalized LLR for `counts` (raw category counts, any category count `>= 2`)
/// evaluating H1 (`elo1`) against H0 (`elo0`), both in logistic Elo. Generic over category
/// count so the same machinery serves both the 5-category pentanomial case and (in tests) a
/// 2-category sanity check against the plain Wald LLR - fixing the arity at 5 here would need
/// more code (hardcoded indices), not less.
fn generalized_llr(counts: &[u64], elo0: f64, elo1: f64) -> f64 {
    let (n, pdf) = results_to_pdf(counts);
    let s0 = crate::stats::sprt::score_from_elo(elo0);
    let s1 = crate::stats::sprt::score_from_elo(elo1);
    n * llr_per_pair(&pdf, s0, s1)
}

/// Pentanomial LLR for `counts = [score_0_0, score_0_5, score_1_0, score_1_5, score_2_0]`
/// (candidate points summed over a pair's two games), evaluating H1 (`elo1`) against H0
/// (`elo0`), both in logistic Elo.
pub(crate) fn pentanomial_llr(elo0: f64, elo1: f64, counts: &[u64; 5]) -> f64 {
    generalized_llr(counts, elo0, elo1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::sprt::{llr_delta, score_from_elo};

    fn assert_close(actual: f64, expected: f64, tol: f64) {
        assert!(
            (actual - expected).abs() < tol,
            "expected {expected}, got {actual}"
        );
    }

    // Strong structural check, not a plausibility test: with exactly 2 categories, the mean
    // constraint alone pins the tilted distribution uniquely (q1 = s, q0 = 1 - s, regardless of
    // the empirical shape - elementary algebra, not an approximation), which makes this reduce
    // *exactly* to `stats::sprt`'s closed-form two-outcome LLR (`win*ln(s1/s0) +
    // loss*ln((1-s1)/(1-s0))` either way), to within ordinary float/bisection rounding.
    // Mirrors `trinomial_sprt.rs`'s own `zero_draws_reduces_to_the_plain_wald_llr` anchor.
    #[test]
    fn two_category_reduces_to_the_plain_wald_llr() {
        let (elo0, elo1) = (0.0, 20.0);
        let (loss, win) = (300u64, 700u64);
        let computed = generalized_llr(&[loss, win], elo0, elo1);

        let (p0, p1) = (score_from_elo(elo0), score_from_elo(elo1));
        let wald_llr =
            win as f64 * llr_delta(true, p0, p1) + loss as f64 * llr_delta(false, p0, p1);

        assert_close(computed, wald_llr, 1e-6);
    }

    #[test]
    fn two_category_reduction_holds_with_a_lopsided_empirical_split() {
        // Same exact-reduction property, but with an empirical win/loss ratio far from the
        // hypothesized means - confirms the reduction isn't an artifact of choosing counts
        // close to score_from_elo(elo0/elo1).
        let (elo0, elo1) = (-10.0, 15.0);
        let (loss, win) = (950u64, 50u64);
        let computed = generalized_llr(&[loss, win], elo0, elo1);

        let (p0, p1) = (score_from_elo(elo0), score_from_elo(elo1));
        let wald_llr =
            win as f64 * llr_delta(true, p0, p1) + loss as f64 * llr_delta(false, p0, p1);

        assert_close(computed, wald_llr, 1e-6);
    }

    // Hand-verified against the actual Fishtest `LLR_logistic` (a direct transcription of
    // `official-stockfish/fishtest`'s `server/fishtest/stats/LLRcalc.py`, run standalone in
    // Python with `scipy.optimize.brentq` for the secular solve - the real reference
    // implementation, not a re-derivation) during implementation - see the plan's P0
    // correctness-anchor requirement. Confirms the actual 5-category port, not just the
    // 2-category reduction above. Both cases are symmetric buckets (empirical mean pair score
    // exactly 0.5, i.e. matching elo0=0/H0 exactly), so a negative LLR is the expected sign:
    // the evidence supports H0 over a positive-elo1 H1.
    #[test]
    fn matches_fishtest_llr_logistic_reference_value() {
        let counts = [5u64, 20, 50, 20, 5];
        let llr = pentanomial_llr(0.0, 10.0, &counts);
        assert_close(llr, -0.206_511_723, 1e-6);
    }

    #[test]
    fn matches_fishtest_llr_logistic_reference_value_draw_heavy() {
        let counts = [2u64, 10, 70, 10, 2];
        let llr = pentanomial_llr(0.0, 20.0, &counts);
        assert_close(llr, -1.523_230_410, 1e-6);
    }

    #[test]
    fn all_mass_in_one_bucket_does_not_panic_or_produce_nan() {
        // The scenario `regularize` exists for: a clean 2-0 sweep across every pair.
        for counts in [[100u64, 0, 0, 0, 0], [0, 0, 0, 0, 100], [0, 100, 0, 0, 0]] {
            let llr = pentanomial_llr(0.0, 20.0, &counts);
            assert!(llr.is_finite(), "counts {counts:?} produced non-finite LLR");
        }
    }

    #[test]
    fn single_pair_of_each_kind_does_not_panic() {
        for i in 0..5 {
            let mut counts = [0u64; 5];
            counts[i] = 1;
            let llr = pentanomial_llr(0.0, 20.0, &counts);
            assert!(llr.is_finite(), "bucket {i} produced non-finite LLR");
        }
    }

    #[test]
    fn stronger_candidate_evidence_drives_llr_upward() {
        // More mass on the winning side (bucket 4) should never produce a *lower* LLR than a
        // more balanced sample, holding total pairs fixed.
        let balanced = pentanomial_llr(0.0, 20.0, &[10, 20, 40, 20, 10]);
        let candidate_favored = pentanomial_llr(0.0, 20.0, &[2, 8, 20, 30, 40]);
        assert!(candidate_favored > balanced);
    }
}
