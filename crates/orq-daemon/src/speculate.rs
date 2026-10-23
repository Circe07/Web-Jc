//! Coordination for speculative verification.
//!
//! After every change the daemon verifies the new tree in the background
//! (clippy first, then the configured `up_to`) and stores each result in the
//! cache, which is keyed by the *content* of the tree (see `orq_cache`). A
//! later `verify` for the same tree is then answered from the cache without
//! running cargo, and one that arrives while the speculative run is still in
//! flight waits for it instead of starting a second run.
//!
//! The cache key is what guarantees a result is never returned for a
//! different tree. This module adds the rest of the correctness argument:
//!
//! - Every change bumps a generation. A speculative result is only recorded
//!   as ready if the generation is the one the run started with, and the
//!   daemon additionally recomputes the cache key after the run and discards
//!   the result if it differs from the key before (an edit landed while
//!   cargo was running, possibly before the watcher reported it).
//! - A change resets readiness and marks the speculation as running in one
//!   atomic step, so a waiter can never observe "ready" for a stale tree.
//! - [`Speculation::finish`] only clears `running` if no change arrived since
//!   the worker last looked, so it cannot hide a newer change.

use orq_core::{Outcome, Step, VerifyResult};
use tokio::sync::watch;

/// What waiters can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpecState {
    /// A speculative run is queued or in flight for the current tree.
    pub(crate) running: bool,
    /// Highest step whose result for the current tree is in the cache.
    pub(crate) ready: Option<Step>,
}

/// Shared speculation state: a change generation and a [`SpecState`].
pub(crate) struct Speculation {
    generation: watch::Sender<u64>,
    state: watch::Sender<SpecState>,
    /// Set by `on_change`, consumed by `take_pending`.
    pending: std::sync::atomic::AtomicBool,
}

impl Speculation {
    pub(crate) fn new() -> Self {
        Self {
            generation: watch::channel(0).0,
            state: watch::channel(SpecState {
                running: false,
                ready: None,
            })
            .0,
            pending: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Records a relevant change to the tree.
    pub(crate) fn on_change(&self) {
        use std::sync::atomic::Ordering;
        // Order matters: `finish` reads `pending` inside the state update.
        self.pending.store(true, Ordering::SeqCst);
        self.generation.send_modify(|g| *g += 1);
        self.state.send_modify(|s| {
            s.running = true;
            s.ready = None;
        });
    }

    /// Consumes the "a change is waiting" flag.
    pub(crate) fn take_pending(&self) -> bool {
        self.pending
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    /// Requests another round without a tree change (a run was abandoned to
    /// let a request through).
    pub(crate) fn requeue(&self) {
        self.pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.state.send_modify(|s| s.running = true);
    }

    /// The current generation.
    pub(crate) fn generation(&self) -> u64 {
        *self.generation.borrow()
    }

    /// Resolves when the generation differs from `seen`. Resolves at once if
    /// it already does, so no change between reading `seen` and calling this
    /// is missed.
    pub(crate) async fn changed_since(&self, seen: u64) {
        let mut rx = self.generation.subscribe();
        // The sender lives as long as `self`.
        let _ = rx.wait_for(|g| *g != seen).await;
    }

    /// Marks results up to `step` as ready, if the tree is still the one of
    /// generation `generation`. Returns whether it was recorded.
    pub(crate) fn mark_ready(&self, generation: u64, step: Step) -> bool {
        let mut recorded = false;
        self.state.send_modify(|s| {
            if *self.generation.borrow() == generation {
                s.ready = Some(s.ready.map_or(step, |r| r.max(step)));
                recorded = true;
            }
        });
        recorded
    }

    /// Ends a round: `running` becomes false unless a change is waiting.
    pub(crate) fn finish(&self) {
        self.state.send_modify(|s| {
            if !self.pending.load(std::sync::atomic::Ordering::SeqCst) {
                s.running = false;
            }
        });
    }

    /// `true` while a run is queued or in flight.
    pub(crate) fn running(&self) -> bool {
        self.state.borrow().running
    }

    /// Waits until the result for `step` of the current tree is, or will not
    /// be, produced by speculation: ready for `step`, or no run in flight.
    /// Whichever the case, the caller then consults the cache, whose key is
    /// authoritative.
    pub(crate) async fn attach(&self, step: Step) {
        let mut rx = self.state.subscribe();
        // The sender lives as long as `self`.
        let _ = rx
            .wait_for(|s| !s.running || s.ready.is_some_and(|r| r >= step))
            .await;
    }
}

/// The result a verification up to `wanted` would give, derived from `result`
/// obtained by running the ladder up to `ran`, or `None` if it cannot be
/// derived (a pass that did not reach `wanted`).
///
/// The ladder stops at the first failing step, so:
/// - a failure at `F` is also the result for every `wanted >= F`;
/// - any run that got past `wanted` has, as its answer for `wanted`, its own
///   steps, diagnostics and outcome truncated to that step (test failures
///   only exist at the Test step).
pub(crate) fn derive(result: &VerifyResult, ran: Step, wanted: Step) -> Option<VerifyResult> {
    match result.outcome {
        Outcome::Fail { failed_step } if wanted >= failed_step => Some(result.clone()),
        Outcome::Pass if wanted > ran => None,
        _ => {
            let mut cut = result.clone();
            cut.outcome = Outcome::Pass;
            cut.steps.retain(|s| s.step <= wanted);
            cut.diagnostics.retain(|d| d.step <= wanted);
            if wanted < Step::Test {
                cut.test_failures.clear();
            }
            Some(cut)
        }
    }
}

/// `true` if the result holds an error that is not a compiler diagnostic
/// (cargo failed to run, network down...). Such results depend on the
/// environment, not on the sources, and must not be replayed.
pub(crate) fn is_tool_failure(result: &VerifyResult) -> bool {
    result
        .diagnostics
        .iter()
        .any(|d| d.severity == orq_core::Severity::Error && d.file.is_none() && d.code.is_none())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use orq_core::{Diagnostic, ImpactSet, Severity, StepReport, TestFailure};

    use super::*;

    fn diag(step: Step, severity: Severity) -> Diagnostic {
        Diagnostic {
            severity,
            code: Some("E0000".into()),
            message: "m".into(),
            file: Some("src/lib.rs".into()),
            line: None,
            column: None,
            snippet: None,
            label: None,
            suggestion: None,
            crate_id: None,
            step,
        }
    }

    fn result(outcome: Outcome, ran: Step) -> VerifyResult {
        VerifyResult {
            outcome,
            steps: Step::ALL
                .into_iter()
                .filter(|s| *s <= ran)
                .map(|step| StepReport {
                    step,
                    duration_ms: 1,
                    passed: true,
                })
                .collect(),
            diagnostics: vec![
                diag(Step::Check, Severity::Warning),
                diag(Step::Clippy, Severity::Warning),
            ],
            test_failures: Vec::new(),
            impact: ImpactSet::default(),
            cache_hit: false,
            total_ms: 5,
        }
    }

    #[test]
    fn pass_derives_lower_steps_truncated() {
        let full = result(Outcome::Pass, Step::Test);
        let at_check = derive(&full, Step::Test, Step::Check).unwrap();
        assert!(at_check.outcome.is_pass());
        assert_eq!(at_check.steps.len(), 2);
        assert_eq!(at_check.diagnostics.len(), 1);
        assert_eq!(at_check.diagnostics[0].step, Step::Check);
        assert_eq!(derive(&full, Step::Test, Step::Test).unwrap(), full);
    }

    #[test]
    fn pass_does_not_cover_higher_steps() {
        let clippy = result(Outcome::Pass, Step::Clippy);
        assert!(derive(&clippy, Step::Clippy, Step::Test).is_none());
        assert!(derive(&clippy, Step::Clippy, Step::Clippy).is_some());
    }

    #[test]
    fn failure_covers_its_step_and_every_higher_one() {
        let failed = Outcome::Fail {
            failed_step: Step::Clippy,
        };
        let r = result(failed, Step::Clippy);
        // Same result at clippy and test: the ladder stops at the failure.
        assert_eq!(derive(&r, Step::Clippy, Step::Clippy).unwrap(), r);
        assert_eq!(derive(&r, Step::Clippy, Step::Test).unwrap(), r);
        // Below the failing step it is a (truncated) pass.
        let check = derive(&r, Step::Clippy, Step::Check).unwrap();
        assert!(check.outcome.is_pass());
        assert!(check.diagnostics.iter().all(|d| d.step <= Step::Check));
    }

    #[test]
    fn test_failures_only_survive_at_test_step() {
        let mut r = result(
            Outcome::Fail {
                failed_step: Step::Test,
            },
            Step::Test,
        );
        r.test_failures.push(TestFailure {
            name: "t".into(),
            crate_id: orq_core::CrateId {
                name: "c".into(),
                manifest_path: "Cargo.toml".into(),
            },
            output: String::new(),
        });
        assert_eq!(derive(&r, Step::Test, Step::Test).unwrap(), r);
        let at_clippy = derive(&r, Step::Test, Step::Clippy).unwrap();
        assert!(at_clippy.outcome.is_pass());
        assert!(at_clippy.test_failures.is_empty());
    }

    #[test]
    fn tool_failures_are_recognised() {
        let mut r = result(Outcome::Pass, Step::Check);
        assert!(!is_tool_failure(&r));
        let mut d = diag(Step::Check, Severity::Error);
        d.file = None;
        d.code = None;
        r.diagnostics.push(d);
        assert!(is_tool_failure(&r));
    }

    #[tokio::test]
    async fn waiter_is_released_only_when_ready_or_idle() {
        let spec = std::sync::Arc::new(Speculation::new());
        // Idle: attach returns at once.
        tokio::time::timeout(Duration::from_secs(1), spec.attach(Step::Test))
            .await
            .unwrap();

        spec.on_change();
        let g = spec.generation();
        let waiter = {
            let spec = std::sync::Arc::clone(&spec);
            tokio::spawn(async move { spec.attach(Step::Test).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "must wait while running");

        // Clippy is ready but the waiter wants Test: still waits.
        assert!(spec.mark_ready(g, Step::Clippy));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished());

        assert!(spec.mark_ready(g, Step::Test));
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn lower_step_waiter_attaches_to_an_earlier_stage() {
        let spec = Speculation::new();
        spec.on_change();
        let g = spec.generation();
        assert!(spec.mark_ready(g, Step::Clippy));
        // Test stage still running, but a check request is already covered.
        assert!(spec.running());
        tokio::time::timeout(Duration::from_secs(1), spec.attach(Step::Check))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn change_during_a_run_invalidates_its_result() {
        let spec = Speculation::new();
        spec.on_change();
        assert!(spec.take_pending());
        let g = spec.generation();
        // The run is in flight; a newer edit arrives.
        spec.on_change();
        // The old run finishes: its result must not be marked ready.
        assert!(!spec.mark_ready(g, Step::Test));
        assert!(!spec.state.borrow().ready.is_some());
        // And finishing the round must not hide the newer change.
        spec.finish();
        assert!(spec.running());
        assert!(spec.take_pending());
        // The rerun for the new tree is recorded normally.
        let g2 = spec.generation();
        assert!(spec.mark_ready(g2, Step::Test));
        spec.finish();
        assert!(!spec.running());
    }

    #[tokio::test]
    async fn change_resets_readiness_for_waiters() {
        let spec = Speculation::new();
        spec.on_change();
        let g = spec.generation();
        assert!(spec.mark_ready(g, Step::Test));
        spec.finish();
        spec.attach(Step::Test).await;
        // A new edit: the old "ready" no longer counts.
        spec.on_change();
        assert_eq!(spec.state.borrow().ready, None);
        let r = tokio::time::timeout(Duration::from_millis(100), spec.attach(Step::Check)).await;
        assert!(r.is_err(), "must wait for the new tree");
    }

    #[tokio::test]
    async fn changed_since_sees_changes_made_before_the_call() {
        let spec = Speculation::new();
        let seen = spec.generation();
        spec.on_change();
        tokio::time::timeout(Duration::from_secs(1), spec.changed_since(seen))
            .await
            .unwrap();
        let r = tokio::time::timeout(
            Duration::from_millis(50),
            spec.changed_since(spec.generation()),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn requeue_keeps_running_and_pending() {
        let spec = Speculation::new();
        spec.on_change();
        assert!(spec.take_pending());
        spec.requeue();
        spec.finish();
        assert!(spec.running(), "a requeued round must not be dropped");
        assert!(spec.take_pending());
    }
}
