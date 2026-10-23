//! Offline, API-free measurement of how much information orq's compact
//! report keeps compared with raw cargo output, and at what token cost.
//!
//! For every case under `eval/cases/<id>/` (see `eval/README.md`) the
//! harness captures raw cargo output and orq's compact report at several
//! budgets, and measures tokens, fact recall, whether the first error shown
//! is the right one, and whether orq blames the right step.

#![warn(missing_docs)]

pub mod capture;
pub mod facts;
pub mod report;
pub mod run;

use std::path::{Path, PathBuf};

/// Errors of the evaluation harness.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    /// A file or directory operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A `case.toml` is invalid.
    #[error("invalid case {path}: {message}")]
    Case {
        /// The `case.toml`.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// Running cargo or orq failed.
    #[error("{0}")]
    Run(String),
}

impl EvalError {
    /// Wraps an I/O error with the path it happened on.
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
