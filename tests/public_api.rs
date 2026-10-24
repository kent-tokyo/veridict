//! Compile-time guard for the public paths retained during internal refactoring.

use veridict::input::{MatchRecord, Record};
use veridict::report::{DataQuality, MultiReport, PairScoreCounts, Report};
use veridict::sprt::{
    PentanomialCounts, SprtAccumulator, SprtCheckpoint, SprtConfig, SprtDataQuality, SprtReport,
    SprtRunOptions, SprtStreamPosition, SprtVariant,
};
use veridict::{
    BootstrapMethod, CiMethod, FailureCaps, FailurePolicy, InconclusiveKind, MatchOutcome,
    MetricConfig, MetricKind, Outcome, Promotion, TrialStatus, Validity, Verdict,
};

#[test]
fn established_public_type_paths_compile() {
    fn assert_public<T>() {}

    assert_public::<Record>();
    assert_public::<MatchRecord>();
    assert_public::<Report>();
    assert_public::<MultiReport>();
    assert_public::<DataQuality>();
    assert_public::<PairScoreCounts>();
    assert_public::<SprtVariant>();
    assert_public::<SprtRunOptions>();
    assert_public::<SprtConfig>();
    assert_public::<PentanomialCounts>();
    assert_public::<SprtReport>();
    assert_public::<SprtDataQuality>();
    assert_public::<SprtStreamPosition>();
    assert_public::<SprtCheckpoint>();
    assert_public::<SprtAccumulator>();
    assert_public::<Verdict>();
    assert_public::<InconclusiveKind>();
    assert_public::<Validity>();
    assert_public::<Promotion>();
    assert_public::<FailureCaps>();
    assert_public::<TrialStatus>();
    assert_public::<Outcome>();
    assert_public::<MatchOutcome>();
    assert_public::<MetricKind>();
    assert_public::<CiMethod>();
    assert_public::<BootstrapMethod>();
    assert_public::<FailurePolicy>();
    assert_public::<MetricConfig>();
}
