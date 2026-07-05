//! Monte Carlo verification of the statistical guarantees this project's docs claim in prose -
//! CI coverage, SPRT error rates, Bradley-Terry rating recovery, and the verdict gate's false-pass
//! rate. Grouped as one integration-test binary (rather than 5 separate ones) so the crate only
//! links once; each area still lives in its own file under `tests/calibration/` for readability.

#[path = "calibration/binomial_coverage.rs"]
mod binomial_coverage;

#[path = "calibration/bootstrap_coverage.rs"]
mod bootstrap_coverage;

#[path = "calibration/sprt_error_rates.rs"]
mod sprt_error_rates;

#[path = "calibration/matrix_bt_recovery.rs"]
mod matrix_bt_recovery;

#[path = "calibration/verdict_false_pass_rate.rs"]
mod verdict_false_pass_rate;
