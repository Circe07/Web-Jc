use std::path::PathBuf;

/// Error shared by every `orq` library crate at contract boundaries.
///
/// Crates may keep richer internal errors and convert into this at the trait
/// boundary.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Filesystem error on a specific path.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// Path that was being accessed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// `cargo metadata` failed or returned something unusable.
    #[error("cargo metadata: {0}")]
    Metadata(String),

    /// A `git` invocation failed.
    #[error("git: {0}")]
    Git(String),

    /// An external process could not be spawned or exited unexpectedly.
    #[error("process `{command}` failed: {message}")]
    Process {
        /// Command line that was run.
        command: String,
        /// What went wrong.
        message: String,
    },

    /// Output from a tool could not be parsed.
    #[error("parse error: {0}")]
    Parse(String),

    /// `orq.toml` is invalid.
    #[error("invalid config: {0}")]
    Config(String),

    /// The result cache could not be read or written.
    #[error("cache: {0}")]
    Cache(String),
}

impl CoreError {
    /// Builds an [`CoreError::Io`] for `path`.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
