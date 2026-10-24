//! Core statistical decision library. Domain-agnostic: consumes baseline vs
//! candidate trial data and returns a pass/fail/inconclusive verdict with
//! enough detail to explain why. No stdout/stderr side effects; that is the
//! CLI's job (see `main.rs`).

pub mod correction;
pub mod error;
pub mod input;
pub mod matrix;
pub mod metrics;
pub mod plan;
pub mod power;
pub mod report;
pub mod sprt;
pub mod stats;
pub mod time_sensitive;
pub mod verdict;
pub mod verify_run;

pub use error::VeridictError;
pub use report::{MultiReport, Report};

mod compare;
mod config;
mod types;

pub use compare::{IntoRecordResult, compare_many, compare_one};
pub use config::MetricConfig;
pub use types::*;

#[cfg(test)]
mod tests;
