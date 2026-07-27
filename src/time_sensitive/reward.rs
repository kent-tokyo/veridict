//! Reward schedules for time-sensitive testing: how much a rejection at time `t` is worth.
//!
//! Every schedule here is non-negative and non-increasing in `t`, with reward at infinite time
//! interpreted as 0 (`R(infinity) = 0`) - a decision that never arrives isn't worth waiting for,
//! no matter how strong the evidence eventually gets. `grid.rs`'s backward induction needs a
//! finite horizon to start from; each variant below documents what its own truncation actually
//! discards, rather than asserting the discarded tail is always negligible.

use crate::error::VeridictError;

#[derive(Debug, Clone)]
pub enum RewardSchedule {
    HardDeadline {
        deadline: u64,
    },
    ExponentialDecay {
        time_scale: f64,
        horizon: u64,
    },
    /// Already-expanded per-`t` values, `rewards[t] = R(t)` for `t` in `0..=horizon()`. Built by
    /// hand for a library caller, or via [`expand_schedule_file`] from the CLI's sparse
    /// breakpoint JSON format.
    Tabulated {
        rewards: Vec<f64>,
    },
}

impl RewardSchedule {
    /// Non-negative, non-increasing, at least one entry - the three properties every reward
    /// schedule must have for "expected reward" to be a well-posed objective at all. Called from
    /// `TimeSensitiveConfig`'s own validation, not automatically on construction (a library
    /// caller building a `Tabulated` schedule by hand may want to construct first, validate
    /// later).
    pub(crate) fn validate(&self) -> Result<(), VeridictError> {
        match self {
            RewardSchedule::HardDeadline { deadline } => {
                if *deadline == 0 {
                    return Err(VeridictError::InvalidThreshold(
                        "--deadline must be >= 1".to_string(),
                    ));
                }
            }
            RewardSchedule::ExponentialDecay {
                time_scale,
                horizon,
            } => {
                if !time_scale.is_finite() || *time_scale <= 0.0 {
                    return Err(VeridictError::InvalidThreshold(format!(
                        "--time-scale must be finite and > 0, got {time_scale}"
                    )));
                }
                if *horizon == 0 {
                    return Err(VeridictError::InvalidThreshold(
                        "--horizon must be >= 1".to_string(),
                    ));
                }
            }
            RewardSchedule::Tabulated { rewards } => {
                // len - 1 (not len) is the schedule's horizon (see `horizon()`) - requiring at
                // least 2 entries keeps that horizon >= 1, the same floor `HardDeadline`/
                // `ExponentialDecay` enforce on their own horizon-shaped parameter.
                if rewards.len() < 2 {
                    return Err(VeridictError::InvalidThreshold(
                        "tabulated reward schedule must have at least 2 entries (t=0 and t=1), \
                         so its horizon is >= 1"
                            .to_string(),
                    ));
                }
                let mut prev = f64::INFINITY;
                for (t, &r) in rewards.iter().enumerate() {
                    if !r.is_finite() || r < 0.0 {
                        return Err(VeridictError::InvalidThreshold(format!(
                            "tabulated reward at t={t} must be finite and >= 0, got {r}"
                        )));
                    }
                    if r > prev {
                        return Err(VeridictError::InvalidThreshold(format!(
                            "tabulated reward schedule must be non-increasing: reward at t={t} \
                             ({r}) exceeds reward at t={} ({prev})",
                            t - 1
                        )));
                    }
                    prev = r;
                }
            }
        }
        Ok(())
    }

    /// Finite truncation horizon for `grid.rs`'s backward induction: the value iteration starts
    /// here and works back to `t=0`, treating "still running at `t=horizon`" as worth exactly
    /// `R(horizon)` (see `grid.rs`'s terminal boundary). `R(t)` for `t` beyond this is never
    /// evaluated - see `truncation_note` for what that actually discards.
    pub fn horizon(&self) -> u64 {
        match self {
            RewardSchedule::HardDeadline { deadline } => *deadline,
            RewardSchedule::ExponentialDecay { horizon, .. } => *horizon,
            RewardSchedule::Tabulated { rewards } => rewards.len() as u64 - 1,
        }
    }

    /// `R(t)`: reward for a rejection landing exactly at time `t`. `t > horizon()` returns 0,
    /// matching the `R(infinity) = 0` convention - this schedule was never going to reward
    /// waiting that long.
    pub fn at(&self, t: u64) -> f64 {
        match self {
            RewardSchedule::HardDeadline { deadline } => {
                if t <= *deadline {
                    1.0
                } else {
                    0.0
                }
            }
            RewardSchedule::ExponentialDecay {
                time_scale,
                horizon,
            } => {
                if t > *horizon {
                    0.0
                } else {
                    (-(t as f64) / time_scale).exp()
                }
            }
            RewardSchedule::Tabulated { rewards } => {
                rewards.get(t as usize).copied().unwrap_or(0.0)
            }
        }
    }

    pub(crate) fn kind_label(&self) -> &'static str {
        match self {
            RewardSchedule::HardDeadline { .. } => "hard_deadline",
            RewardSchedule::ExponentialDecay { .. } => "exponential_decay",
            RewardSchedule::Tabulated { .. } => "tabulated",
        }
    }

    /// Caller-facing parameters, for `TimeSensitiveReport::reward_parameters`.
    pub(crate) fn parameters(&self) -> serde_json::Value {
        match self {
            RewardSchedule::HardDeadline { deadline } => {
                serde_json::json!({ "deadline": deadline })
            }
            RewardSchedule::ExponentialDecay {
                time_scale,
                horizon,
            } => {
                serde_json::json!({ "time_scale": time_scale, "horizon": horizon })
            }
            RewardSchedule::Tabulated { rewards } => {
                serde_json::json!({ "horizon": self.horizon(), "entries": rewards.len() })
            }
        }
    }

    /// One sentence on what this schedule's finite-horizon truncation actually discards -
    /// `HardDeadline` truncates exactly (reward really is 0 past the deadline, not an
    /// approximation of anything); `ExponentialDecay`/`Tabulated` report the residual reward
    /// value at the horizon so a caller can judge negligibility for their own alpha/horizon
    /// choice, rather than veridict silently asserting it always is.
    pub(crate) fn truncation_note(&self) -> String {
        match self {
            RewardSchedule::HardDeadline { deadline } => format!(
                "hard-deadline reward is exact: R(t) = 0 for t > {deadline} by construction, not \
                 a truncation approximation"
            ),
            RewardSchedule::ExponentialDecay { horizon, .. } => {
                let residual = self.at(*horizon);
                format!(
                    "exponential-decay reward truncated at horizon={horizon}: R(horizon) = \
                     {residual:.3e} against R(0) = 1.0 is discarded for t > horizon; judge \
                     negligibility against your own horizon-to-time_scale ratio before trusting \
                     planned_expected_reward_under_p1 at a small ratio"
                )
            }
            RewardSchedule::Tabulated { rewards } => {
                let horizon = self.horizon();
                let last = *rewards.last().unwrap_or(&0.0);
                format!(
                    "tabulated reward truncated at horizon={horizon}: last declared value is \
                     {last}; R(t) = 0 for t > horizon"
                )
            }
        }
    }
}

/// One entry of a user-supplied reward-schedule JSON file (`--reward-schedule`). Exactly one of
/// `until`/`after` is set per entry: `until` declares "value holds for t <= until" (there may be
/// several, in any order), `after` declares the tail beyond the largest `until`. Kept as a
/// separate on-disk shape from `RewardSchedule::Tabulated`'s already-expanded `Vec<f64>` - the
/// sparse breakpoint form is what a human writes, the dense per-`t` form is what `grid.rs` needs.
#[derive(Debug, serde::Deserialize)]
pub struct RewardScheduleEntry {
    pub until: Option<u64>,
    pub after: Option<u64>,
    pub value: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct RewardScheduleFile {
    pub rewards: Vec<RewardScheduleEntry>,
}

/// Expands a sparse breakpoint list into `RewardSchedule::Tabulated`'s dense per-`t` form.
///
/// The tail (`after`) value is required to be exactly `0.0`: the module's `R(infinity) = 0`
/// invariant has no finite representation of "stays worth `V` forever" for `V != 0`, so a
/// nonzero tail isn't a value this schedule shape can express, not merely an unusual one.
pub fn expand_schedule_file(file: &RewardScheduleFile) -> Result<RewardSchedule, VeridictError> {
    let mut untils: Vec<(u64, f64)> = Vec::new();
    let mut after: Option<f64> = None;
    for entry in &file.rewards {
        match (entry.until, entry.after) {
            (Some(u), None) => untils.push((u, entry.value)),
            (None, Some(_)) if after.is_some() => {
                return Err(VeridictError::InvalidThreshold(
                    "reward schedule file must declare at most one 'after' entry".to_string(),
                ));
            }
            (None, Some(_)) => after = Some(entry.value),
            _ => {
                return Err(VeridictError::InvalidThreshold(
                    "each reward schedule entry must set exactly one of 'until'/'after'"
                        .to_string(),
                ));
            }
        }
    }
    let Some(after) = after else {
        return Err(VeridictError::InvalidThreshold(
            "reward schedule file must declare exactly one 'after' entry for the tail value"
                .to_string(),
        ));
    };
    if after != 0.0 {
        return Err(VeridictError::InvalidThreshold(format!(
            "reward schedule file's 'after' entry must be 0.0 (got {after}): R(t) as t -> \
             infinity must be 0 by definition, so a schedule that stays rewarding forever has no \
             well-defined finite-horizon Bellman problem"
        )));
    }
    if untils.is_empty() {
        return Err(VeridictError::InvalidThreshold(
            "reward schedule file must declare at least one 'until' entry".to_string(),
        ));
    }
    untils.sort_by_key(|(u, _)| *u);
    for pair in untils.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(VeridictError::InvalidThreshold(format!(
                "reward schedule file declares 'until: {}' more than once",
                pair[0].0
            )));
        }
    }
    let horizon = untils.last().unwrap().0;
    let mut rewards = vec![after; horizon as usize + 1];
    // Fill widest-range (largest `until`) first, then let each narrower, smaller-`until` entry
    // overwrite its own prefix - matches the format's "first matching until wins" reading of a
    // breakpoint list (see the spec's own 400/1600/3200 example).
    for &(until, value) in untils.iter().rev() {
        for slot in rewards.iter_mut().take(until as usize + 1) {
            *slot = value;
        }
    }
    let schedule = RewardSchedule::Tabulated { rewards };
    schedule.validate()?;
    Ok(schedule)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_deadline_is_one_then_zero() {
        let s = RewardSchedule::HardDeadline { deadline: 400 };
        assert_eq!(s.at(0), 1.0);
        assert_eq!(s.at(400), 1.0);
        assert_eq!(s.at(401), 0.0);
        assert_eq!(s.horizon(), 400);
        s.validate().unwrap();
    }

    #[test]
    fn hard_deadline_zero_is_rejected() {
        assert!(
            RewardSchedule::HardDeadline { deadline: 0 }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn exponential_decay_is_monotonic_and_positive() {
        let s = RewardSchedule::ExponentialDecay {
            time_scale: 800.0,
            horizon: 3200,
        };
        s.validate().unwrap();
        assert_eq!(s.at(0), 1.0);
        assert!(s.at(800) < s.at(0));
        assert!(s.at(3200) < s.at(800));
        assert_eq!(s.at(3201), 0.0);
    }

    #[test]
    fn exponential_decay_rejects_non_positive_time_scale() {
        assert!(
            RewardSchedule::ExponentialDecay {
                time_scale: 0.0,
                horizon: 100,
            }
            .validate()
            .is_err()
        );
        assert!(
            RewardSchedule::ExponentialDecay {
                time_scale: -1.0,
                horizon: 100,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn tabulated_rejects_negative_reward() {
        let s = RewardSchedule::Tabulated {
            rewards: vec![1.0, -0.1],
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn tabulated_rejects_increasing_reward() {
        let s = RewardSchedule::Tabulated {
            rewards: vec![0.1, 0.4],
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn expand_schedule_file_matches_spec_example() {
        let file = RewardScheduleFile {
            rewards: vec![
                RewardScheduleEntry {
                    until: Some(400),
                    after: None,
                    value: 1.0,
                },
                RewardScheduleEntry {
                    until: Some(1600),
                    after: None,
                    value: 0.4,
                },
                RewardScheduleEntry {
                    until: Some(3200),
                    after: None,
                    value: 0.1,
                },
                RewardScheduleEntry {
                    until: None,
                    after: Some(3200),
                    value: 0.0,
                },
            ],
        };
        let schedule = expand_schedule_file(&file).unwrap();
        assert_eq!(schedule.horizon(), 3200);
        assert_eq!(schedule.at(0), 1.0);
        assert_eq!(schedule.at(400), 1.0);
        assert_eq!(schedule.at(401), 0.4);
        assert_eq!(schedule.at(1600), 0.4);
        assert_eq!(schedule.at(1601), 0.1);
        assert_eq!(schedule.at(3200), 0.1);
        assert_eq!(schedule.at(3201), 0.0);
    }

    #[test]
    fn expand_schedule_file_rejects_nonzero_tail() {
        let file = RewardScheduleFile {
            rewards: vec![
                RewardScheduleEntry {
                    until: Some(100),
                    after: None,
                    value: 1.0,
                },
                RewardScheduleEntry {
                    until: None,
                    after: Some(100),
                    value: 0.2,
                },
            ],
        };
        assert!(expand_schedule_file(&file).is_err());
    }

    #[test]
    fn expand_schedule_file_requires_an_after_entry() {
        let file = RewardScheduleFile {
            rewards: vec![RewardScheduleEntry {
                until: Some(100),
                after: None,
                value: 1.0,
            }],
        };
        assert!(expand_schedule_file(&file).is_err());
    }
}
