//! Applies the machine-applicable suggestions of rustc and clippy
//! (`orq fix`, MCP `apply_fix`; PLAN.md T5.3).

use std::path::{Path, PathBuf};

use orq_core::{BuildOptions, ImpactSet};

use crate::RunnerError;

/// What [`apply_fixes`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixOutcome {
    /// Files modified, relative to the workspace root, sorted.
    pub files_changed: Vec<PathBuf>,
}

/// Applies machine-applicable fixes to the crates of `impact`.
///
/// Stub until PLAN.md T5.3 implements it.
pub async fn apply_fixes(
    root: &Path,
    impact: &ImpactSet,
    build: &BuildOptions,
) -> Result<FixOutcome, RunnerError> {
    let _ = (root, impact, build);
    Err(RunnerError::Unsupported(
        "`orq fix` is not implemented yet (PLAN.md T5.3)".into(),
    ))
}
