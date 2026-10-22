//! Shared contracts for `orq`.
//!
//! Every other crate in the workspace depends on the types and traits defined
//! here. Only the lead agent changes this crate (see `PLAN.md` §1); if another
//! crate needs a contract change it must be requested, not made.

#![warn(missing_docs)]

mod config;
mod error;
pub mod ipc;
mod pipeline;
mod types;

pub use config::{Config, Lang, NextestMode, CONFIG_FILE_NAME};
pub use error::CoreError;
pub use pipeline::Pipeline;
pub use types::*;

use std::future::Future;
use std::path::Path;

/// Default token budget for a [`CompactReport`].
pub const DEFAULT_TOKEN_BUDGET: usize = 1500;

/// Version of `orq`, folded into cache keys so upgrades invalidate old results.
pub const ORQ_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Rough token estimate for LLM output: `ceil(chars / 4)`.
///
/// Counts Unicode scalar values, not bytes, so non-ASCII text is not
/// over-estimated.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Maps changed files to the crates they affect.
pub trait ImpactAnalyzer: Send + Sync {
    /// Computes the [`ImpactSet`] for `changes` inside the workspace at `root`.
    ///
    /// Paths in `changes` are relative to `root`. Files outside every crate
    /// are ignored.
    fn analyze(&self, root: &Path, changes: &ChangeSet) -> Result<ImpactSet, CoreError>;

    /// Detects the changed files of the workspace at `root` (used when a
    /// [`VerifyRequest`] has no explicit `changes`).
    fn detect_changes(&self, root: &Path) -> Result<ChangeSet, CoreError>;
}

/// Runs the verification ladder on the crates of an [`ImpactSet`].
pub trait StepRunner: Send + Sync {
    /// Runs steps in order up to `up_to`, stopping at the first one with
    /// errors. The result has `cache_hit = false` and `impact = impact`.
    fn run(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
    ) -> impl Future<Output = Result<VerifyResult, CoreError>> + Send;
}

/// Turns a [`VerifyResult`] into a compact report for an LLM.
pub trait Compressor: Send + Sync {
    /// Compresses `result` so that [`CompactReport::estimated_tokens`] never
    /// exceeds `budget`.
    fn compress(&self, result: &VerifyResult, budget: usize) -> CompactReport;
}

/// Stores verification results keyed by the relevant workspace state.
pub trait ResultCache: Send + Sync {
    /// Computes the key for the current state of the crates in `impact`,
    /// verified up to `up_to`.
    fn key(&self, root: &Path, impact: &ImpactSet, up_to: Step) -> Result<CacheKey, CoreError>;

    /// Returns the stored result for `key`, if any.
    fn get(&self, key: &CacheKey) -> Result<Option<VerifyResult>, CoreError>;

    /// Stores `result` under `key`, replacing any previous entry.
    fn put(&self, key: &CacheKey, result: &VerifyResult) -> Result<(), CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn estimate_tokens_counts_chars_not_bytes() {
        assert_eq!(estimate_tokens("ñañá"), 1);
    }
}
