//! Bellman-grid policy: a numerical approximation of the paper's Bellman recursion on a finite
//! `(t, log_wealth)` grid, for a general (not necessarily exponential-decay) reward schedule. See
//! `grid.rs`'s doc for why grid/action-resolution approximation error here can only ever cost
//! optimality, never anytime-validity.
//!
//! Deliberately never called "Bellman-optimal" anywhere in this module, its report fields, or its
//! docs: it is a numerical approximation on a specific `(action_grid_size, wealth_grid_size)`
//! grid, not a proof of optimality on the true continuous action/wealth space - `method` in
//! `TimeSensitiveReport` reads `bellman_grid_approximation` for exactly this reason.

use super::e_variable::BernoulliHypotheses;
use super::grid::{self, WealthGrid};
use super::reward::RewardSchedule;

pub(crate) struct BellmanGrid {
    policy: Vec<Vec<u32>>,
    actions: Vec<f64>,
    grid: WealthGrid,
    planned_value: f64,
}

impl BellmanGrid {
    pub(crate) fn build(
        hypotheses: &BernoulliHypotheses,
        alpha: f64,
        reward: &RewardSchedule,
        wealth_grid_size: usize,
        action_grid_size: usize,
    ) -> Self {
        let grid = WealthGrid::new(alpha, wealth_grid_size);
        let actions = grid::action_grid(action_grid_size);
        let result = grid::maximize(hypotheses.p0, hypotheses.p1, reward, &grid, &actions);
        Self {
            policy: result.policy,
            actions,
            grid,
            planned_value: result.planned_value,
        }
    }

    pub(crate) fn planned_value(&self) -> f64 {
        self.planned_value
    }

    /// Nearest-grid-cell policy lookup - "an action must come from a real grid point," not an
    /// interpolated blend of actions (see `WealthGrid::nearest_index`'s doc). `t >= horizon` (the
    /// process outlived its own policy table without rejecting) has no stored row;
    /// `TimeSensitiveTest` never calls this once `t` reaches the schedule's horizon, but this
    /// falls back to the last computed row rather than panic if it ever did.
    pub(crate) fn action(&self, t: u64, log_wealth: f64) -> f64 {
        let row = self.policy.get(t as usize).unwrap_or_else(|| {
            self.policy
                .last()
                .expect("horizon >= 1, policy is non-empty")
        });
        let index = self.grid.nearest_index(log_wealth);
        self.actions[row[index] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyp() -> BernoulliHypotheses {
        BernoulliHypotheses { p0: 0.5, p1: 0.55 }
    }

    #[test]
    fn action_is_always_a_real_grid_action() {
        let reward = RewardSchedule::HardDeadline { deadline: 40 };
        let bellman = BellmanGrid::build(&hyp(), 0.05, &reward, 60, 41);
        for t in 0..40u64 {
            for &w in &[-3.0, -1.0, 0.0, 1.0, 2.5] {
                let a = bellman.action(t, w);
                assert!(bellman.actions.contains(&a));
                assert!(a > 0.0 && a < 1.0);
            }
        }
    }

    #[test]
    fn more_time_never_decreases_planned_value() {
        // A longer deadline can only weakly help: any policy valid under the shorter deadline is
        // still available under the longer one (the extra trials at the tail are simply never
        // required to matter). A provable monotonicity property, not a simulated observation.
        let short = RewardSchedule::HardDeadline { deadline: 20 };
        let long = RewardSchedule::HardDeadline { deadline: 40 };
        let short_value = BellmanGrid::build(&hyp(), 0.05, &short, 80, 61).planned_value();
        let long_value = BellmanGrid::build(&hyp(), 0.05, &long, 80, 61).planned_value();
        assert!(long_value >= short_value - 1e-6);
    }
}
