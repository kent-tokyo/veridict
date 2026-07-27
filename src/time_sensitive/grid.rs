//! Shared backward value-iteration core over `(t, log_wealth)`, used three ways:
//!
//! 1. [`maximize`] - Bellman policy construction: maximize over the action grid at every cell,
//!    producing a policy table (`bellman.rs`).
//! 2. [`evaluate_fixed_action`] - pin the action to a single constant every trial instead of
//!    maximizing; this is how GRO's and EDO's `planned_expected_reward_under_p1` get computed
//!    *exactly* (no Monte Carlo), and how the "Bellman >= GRO" calibration check gets an exact
//!    number to compare against instead of a simulated one.
//!
//! Approximation error here (grid resolution, floor truncation, linear interpolation) only ever
//! affects which numbers these two functions return - it can never affect anytime-validity. See
//! `e_variable::phi`'s doc: any `a` in `(0,1)`, however it was chosen, keeps the wealth update a
//! valid H0-martingale step. What grid resolution *does* affect is optimality - how close the
//! reported/executed policy comes to the true (uncomputable in closed form, for a general
//! schedule) optimal one.

use super::e_variable::{ACTION_EPSILON, log_phi};
use super::reward::RewardSchedule;

/// How far below the wealth threshold the grid's floor sits, in nats (log-wealth units).
/// `exp(-FLOOR_MARGIN_NATS)` is the floor's wealth relative to the threshold - at 50 nats that's
/// on the order of `1e-22`, a wealth level no realistic `alpha`/horizon combination has any
/// meaningful probability of both reaching *and* recovering from before the horizon. Successor
/// states below the floor are clamped to it (see `WealthGrid::interpolate`); the clamp only
/// biases the value function for paths already this deep in loss territory, whose probability
/// mass and reward contribution under either `p0` or `p1` is negligible for any config this
/// module's numerical-boundary tests accept.
const FLOOR_MARGIN_NATS: f64 = 50.0;

/// Backward-induction cost estimate (`horizon * wealth_grid_size * action_grid_size`), the same
/// quantity `TimeSensitiveConfig::validate` checks against a cap before running anything -
/// exposed here so that check and the recursion it's estimating stay next to each other.
pub(crate) fn estimate_ops(horizon: u64, wealth_grid_size: usize, action_grid_size: usize) -> u128 {
    horizon as u128 * wealth_grid_size as u128 * action_grid_size as u128
}

/// Log-wealth grid: `wealth_grid_size` points linearly spaced from a floor up to (but not
/// including) `log_threshold = ln(1/alpha)`, which is the absorbing rejection boundary, not a
/// grid cell in its own right - a successor landing at or past it is valued at the reward
/// schedule directly (see `maximize`/`evaluate_fixed_action`), never interpolated.
pub(crate) struct WealthGrid {
    floor: f64,
    log_threshold: f64,
    size: usize,
    step: f64,
}

impl WealthGrid {
    pub(crate) fn new(alpha: f64, size: usize) -> Self {
        debug_assert!(
            size >= 2,
            "wealth_grid_size must be >= 2 (validated by caller)"
        );
        let log_threshold = (1.0 / alpha).ln();
        let floor = log_threshold - FLOOR_MARGIN_NATS;
        let step = (log_threshold - floor) / (size - 1) as f64;
        Self {
            floor,
            log_threshold,
            size,
            step,
        }
    }

    pub(crate) fn log_threshold(&self) -> f64 {
        self.log_threshold
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }

    /// The grid's log-wealth spacing - reported as `TimeSensitiveReport::numerical_tolerance`,
    /// the dominant source of numerical imprecision in `planned_expected_reward_under_p1` (and,
    /// for `bellman`, in the executed policy itself) across all three policy kinds.
    pub(crate) fn step(&self) -> f64 {
        self.step
    }

    pub(crate) fn point(&self, i: usize) -> f64 {
        self.floor + self.step * i as f64
    }

    /// Linear interpolation of a value array sampled on this grid at an arbitrary `log_wealth`
    /// (generally off-grid: a successor `w + log_phi(...)` almost never lands exactly on a grid
    /// point). Below the floor, clamps to the floor's own value (see `FLOOR_MARGIN_NATS`'s doc);
    /// at/above `log_threshold` is a caller error - the caller must have already routed that case
    /// to the reward schedule directly, since this grid holds only sub-threshold ("not yet
    /// rejected") states.
    pub(crate) fn interpolate(&self, values: &[f64], w: f64) -> f64 {
        debug_assert_eq!(values.len(), self.size);
        debug_assert!(
            w < self.log_threshold,
            "caller must route >= threshold to R(t) directly"
        );
        if w <= self.floor {
            return values[0];
        }
        let pos = (w - self.floor) / self.step;
        let i = (pos.floor() as usize).min(self.size - 2);
        let frac = (pos - i as f64).clamp(0.0, 1.0);
        values[i] * (1.0 - frac) + values[i + 1] * frac
    }

    /// Nearest grid index to `w` - used only for Bellman's *action* lookup at runtime (never for
    /// a value, which always goes through `interpolate`): the spec's "an action must come from a
    /// real grid point," not an interpolated blend of two actions.
    pub(crate) fn nearest_index(&self, w: f64) -> usize {
        if w <= self.floor {
            return 0;
        }
        if w >= self.log_threshold {
            return self.size - 1;
        }
        (((w - self.floor) / self.step).round() as usize).min(self.size - 1)
    }
}

/// `action_grid_size` points evenly spaced in `(ACTION_EPSILON, 1 - ACTION_EPSILON)`.
pub(crate) fn action_grid(size: usize) -> Vec<f64> {
    debug_assert!(
        size >= 2,
        "action_grid_size must be >= 2 (validated by caller)"
    );
    let lo = ACTION_EPSILON;
    let hi = 1.0 - ACTION_EPSILON;
    (0..size)
        .map(|i| lo + (hi - lo) * i as f64 / (size - 1) as f64)
        .collect()
}

/// One successor-value lookup: `R(t+1)` if the successor log-wealth has crossed the threshold,
/// else the interpolated continuation value. Shared by `maximize` and `evaluate_fixed_action` so
/// the "which side of the threshold" logic can't drift between the two.
fn successor_value(grid: &WealthGrid, next_values: &[f64], w_successor: f64, r_next: f64) -> f64 {
    if w_successor >= grid.log_threshold() {
        r_next
    } else {
        grid.interpolate(next_values, w_successor)
    }
}

pub(crate) struct MaximizeResult {
    /// `policy[t][wealth_index]` = index into the action grid of the argmax action at `(t,
    /// wealth_index)`, for `t` in `0..horizon`. `O(horizon * wealth_grid_size)`, matching the
    /// module's documented memory target for a stored policy table.
    pub(crate) policy: Vec<Vec<u32>>,
    /// `V_0` interpolated at `log_wealth = 0` (wealth = 1, the start of the process) - the
    /// Bellman policy's own `planned_expected_reward_under_p1`.
    pub(crate) planned_value: f64,
}

/// Backward value iteration, maximizing over `actions` at every `(t, wealth_index)` cell.
/// Only two value rows (`next`/`current`, each `O(wealth_grid_size)`) are ever live at once; the
/// policy table is the only structure kept for the full horizon (see `MaximizeResult`'s doc).
pub(crate) fn maximize(
    p0: f64,
    p1: f64,
    reward: &RewardSchedule,
    grid: &WealthGrid,
    actions: &[f64],
) -> MaximizeResult {
    let horizon = reward.horizon();
    let size = grid.size();
    // Terminal boundary: `V_horizon(w) = 0` for every still-live `w`, matching
    // `RewardSchedule::at`'s own convention that `R(t) = 0` for every `t > horizon` (see its
    // doc). This is exact, not approximate, *given that convention*: a path still not crossed
    // at the horizon has no more decision budget, and every reward this schedule could still
    // offer from here on is 0 by construction. The approximation is one level up, in choosing
    // `horizon` at all for a schedule (like `ExponentialDecay`) whose *true*, untruncated tail
    // is small-but-nonzero rather than exactly 0 - `RewardSchedule::truncation_note` reports
    // that discarded residual (`R(horizon)`, the scale of what's being rounded down to 0) so a
    // caller can judge it, rather than this recursion silently smuggling a nonzero "free" reward
    // into every not-yet-crossed terminal state (which, for a schedule like `HardDeadline` where
    // reward truly is 0 immediately past the deadline, would be a hard error, not a mere
    // approximation: it would make the policy indifferent to ever crossing at all).
    let mut next = vec![0.0; size];
    let mut policy = vec![Vec::new(); horizon as usize];

    // Ties default to the action closest to `p1` (GRO's own choice), not the action grid's
    // arbitrary first entry: in a state where crossing before the horizon is already impossible
    // regardless of action (common deep in loss territory near/below the wealth floor - see
    // `FLOOR_MARGIN_NATS`'s doc), *every* action has identical value (0), so which one "wins" the
    // argmax is genuinely arbitrary for optimality but not for behavior - a plain `>` scan
    // seeded at index 0 would silently default to betting near 0 (a near-certain, and pointless,
    // loss on every future success). Seeding the scan with `p1`'s own action instead means a
    // value-tied, already-doomed state keeps betting like GRO rather than snapping to the
    // grid's smallest action, without changing `best_value` (and so `planned_value`/optimality)
    // at all.
    let gro_action_index = actions
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (**a - p1).abs().total_cmp(&(**b - p1).abs()))
        .map(|(i, _)| i as u32)
        .unwrap_or(0);

    for t in (0..horizon).rev() {
        let r_next = reward.at(t + 1);
        let mut current = vec![0.0; size];
        let mut current_policy = vec![0u32; size];
        for wi in 0..size {
            let w = grid.point(wi);
            let seed_action = actions[gro_action_index as usize];
            let seed_value = {
                let w_success = w + log_phi(seed_action, p0, true);
                let w_failure = w + log_phi(seed_action, p0, false);
                let v_success = successor_value(grid, &next, w_success, r_next);
                let v_failure = successor_value(grid, &next, w_failure, r_next);
                p1 * v_success + (1.0 - p1) * v_failure
            };
            let mut best_value = seed_value;
            let mut best_action_index = gro_action_index;
            for (ai, &a) in actions.iter().enumerate() {
                let w_success = w + log_phi(a, p0, true);
                let w_failure = w + log_phi(a, p0, false);
                let v_success = successor_value(grid, &next, w_success, r_next);
                let v_failure = successor_value(grid, &next, w_failure, r_next);
                let value = p1 * v_success + (1.0 - p1) * v_failure;
                if value > best_value {
                    best_value = value;
                    best_action_index = ai as u32;
                }
            }
            current[wi] = best_value;
            current_policy[wi] = best_action_index;
        }
        policy[t as usize] = current_policy;
        next = current;
    }

    let planned_value = grid.interpolate(&next, 0.0);
    MaximizeResult {
        policy,
        planned_value,
    }
}

/// Same recursion as [`maximize`], but the action is pinned to a single constant every trial
/// instead of maximized - a *policy evaluation* pass, not a *policy improvement* pass. Returns
/// `V_0` interpolated at `log_wealth = 0`: the exact (not simulated) expected reward of betting
/// `action` every trial under `p1`, for the given schedule/horizon.
pub(crate) fn evaluate_fixed_action(
    p0: f64,
    p1: f64,
    reward: &RewardSchedule,
    grid: &WealthGrid,
    action: f64,
) -> f64 {
    let horizon = reward.horizon();
    let size = grid.size();
    // See `maximize`'s doc for why the terminal boundary is exactly 0, not `R(horizon)`.
    let mut next = vec![0.0; size];
    let w_success_delta = log_phi(action, p0, true);
    let w_failure_delta = log_phi(action, p0, false);

    for t in (0..horizon).rev() {
        let r_next = reward.at(t + 1);
        let mut current = vec![0.0; size];
        for (wi, slot) in current.iter_mut().enumerate() {
            let w = grid.point(wi);
            let v_success = successor_value(grid, &next, w + w_success_delta, r_next);
            let v_failure = successor_value(grid, &next, w + w_failure_delta, r_next);
            *slot = p1 * v_success + (1.0 - p1) * v_failure;
        }
        next = current;
    }

    grid.interpolate(&next, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_grid_stays_strictly_inside_unit_interval() {
        let grid = action_grid(11);
        assert_eq!(grid.len(), 11);
        assert!(grid[0] > 0.0 && grid[0] < 1.0);
        assert!(grid[grid.len() - 1] > 0.0 && grid[grid.len() - 1] < 1.0);
        assert!(grid.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn wealth_grid_interpolation_is_exact_on_grid_points() {
        let grid = WealthGrid::new(0.05, 50);
        let values: Vec<f64> = (0..grid.size()).map(|i| i as f64 * 2.0).collect();
        for i in 0..grid.size() {
            let w = grid.point(i);
            assert!((grid.interpolate(&values, w) - values[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn wealth_grid_interpolation_is_linear_between_points() {
        let grid = WealthGrid::new(0.05, 10);
        let values: Vec<f64> = (0..grid.size()).map(|i| i as f64).collect();
        let midpoint = (grid.point(3) + grid.point(4)) / 2.0;
        assert!((grid.interpolate(&values, midpoint) - 3.5).abs() < 1e-9);
    }

    #[test]
    fn wealth_grid_clamps_below_floor() {
        let grid = WealthGrid::new(0.05, 10);
        let values: Vec<f64> = (0..grid.size()).map(|i| i as f64).collect();
        assert_eq!(grid.interpolate(&values, grid.floor - 100.0), values[0]);
    }

    #[test]
    fn evaluate_fixed_action_stays_within_reward_bounds() {
        let reward = RewardSchedule::HardDeadline { deadline: 50 };
        let grid = WealthGrid::new(0.05, 100);
        let value = evaluate_fixed_action(0.5, 0.55, &reward, &grid, 0.55);
        assert!((0.0..=1.0).contains(&value));
    }

    #[test]
    fn maximize_planned_value_is_at_least_gro_fixed_action_value() {
        let reward = RewardSchedule::HardDeadline { deadline: 50 };
        let grid = WealthGrid::new(0.05, 100);
        let actions = action_grid(101);
        let bellman = maximize(0.5, 0.55, &reward, &grid, &actions);
        let gro = evaluate_fixed_action(0.5, 0.55, &reward, &grid, 0.55);
        assert!(bellman.planned_value >= gro - 1e-9);
    }

    #[test]
    fn ties_in_a_doomed_state_default_to_the_gro_action_not_the_grid_endpoint() {
        // 1 trial left, starting from the wealth grid's deepest (floor) cell: no action can
        // possibly cross before the deadline (max single-step gain, ln(1/p0), is nowhere near
        // FLOOR_MARGIN_NATS), so every action's value is tied at 0. Regression test for a bug
        // where that tie silently defaulted to the action grid's smallest entry (bet ~0, making
        // every future *success* catastrophic) instead of the GRO/p1 action - see `maximize`'s
        // tie-breaking comment.
        let p0 = 0.5;
        let p1 = 0.55;
        let reward = RewardSchedule::HardDeadline { deadline: 1 };
        let grid = WealthGrid::new(0.05, 50);
        let actions = action_grid(41);
        let result = maximize(p0, p1, &reward, &grid, &actions);
        let chosen = actions[result.policy[0][0] as usize];
        assert!(
            (chosen - p1).abs() < 1e-6,
            "chosen={chosen}, expected p1={p1}"
        );
    }
}
