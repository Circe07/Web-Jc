use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{BuildOptions, CoreError, Step, DEFAULT_TOKEN_BUDGET};

/// Name of the per-project config file, read from the workspace root.
pub const CONFIG_FILE_NAME: &str = "orq.toml";

/// Language of the compact report text.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// English (default; fewer tokens).
    #[default]
    En,
    /// Spanish.
    Es,
}

/// Whether to run tests with `cargo nextest`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NextestMode {
    /// Use nextest if installed, otherwise `cargo test`.
    #[default]
    Auto,
    /// Always use nextest (error if missing).
    Always,
    /// Always use `cargo test`.
    Never,
}

/// Project configuration from `orq.toml`. Every field is optional.
///
/// ```toml
/// budget = 1500
/// up_to = "test"
/// lang = "en"
/// nextest = "auto"
/// ignore = ["docs/**"]
/// autostart_daemon = true
/// deny_warnings = false
///
/// [build]
/// features = ["serde"]
/// all_features = false
/// no_default_features = false
/// target = "x86_64-unknown-linux-gnu"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Token budget per response.
    pub budget: usize,
    /// Default last step of the ladder.
    pub up_to: Step,
    /// Report language.
    pub lang: Lang,
    /// Test runner selection.
    pub nextest: NextestMode,
    /// Glob patterns (relative to the root) whose changes are ignored.
    /// Supports `*` (within a path segment) and `**` (any number of segments).
    pub ignore: Vec<String>,
    /// Whether `orq mcp` starts a daemon for the workspace when none is
    /// running (`ORQ_NO_DAEMON=1` disables daemons altogether).
    pub autostart_daemon: bool,
    /// Treat compiler and clippy warnings as errors (`clippy -- -D warnings`)
    /// so they stop the ladder. Applies when the clippy step runs.
    pub deny_warnings: bool,
    /// Default feature/target selection.
    pub build: BuildOptions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            budget: DEFAULT_TOKEN_BUDGET,
            up_to: Step::Test,
            lang: Lang::En,
            nextest: NextestMode::Auto,
            ignore: Vec::new(),
            autostart_daemon: true,
            deny_warnings: false,
            build: BuildOptions::default(),
        }
    }
}

impl Config {
    /// Parses a config from TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self, CoreError> {
        toml::from_str(text).map_err(|e| CoreError::Config(e.to_string()))
    }

    /// Loads `<root>/orq.toml`, or returns the defaults if it does not exist.
    pub fn load(root: &Path) -> Result<Self, CoreError> {
        let path = root.join(CONFIG_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml_str(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// `true` if `path` (relative to the root, `/`-separated) matches any
    /// pattern in [`Config::ignore`].
    pub fn is_ignored(&self, path: &Path) -> bool {
        let path = path.to_string_lossy().replace('\\', "/");
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        self.ignore.iter().any(|pattern| {
            let pattern: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
            glob_segments(&pattern, &segments)
        })
    }
}

/// Matches path segments against pattern segments (`**` = any segments).
fn glob_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| glob_segments(rest, &path[i..])),
        Some((first, rest)) => match path.split_first() {
            Some((segment, path_rest)) => {
                glob_segment(first.as_bytes(), segment.as_bytes()) && glob_segments(rest, path_rest)
            }
            None => false,
        },
    }
}

/// Matches one segment with `*` (any run of characters) and `?` (one byte).
fn glob_segment(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|i| glob_segment(rest, &text[i..])),
        Some((&p, rest)) => match text.split_first() {
            Some((&t, text_rest)) => (p == b'?' || p == t) && glob_segment(rest, text_rest),
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::from_toml_str("").unwrap(), Config::default());
    }

    #[test]
    fn parses_plan_example() {
        let config = Config::from_toml_str(
            r#"
            budget = 800
            up_to = "clippy"
            lang = "es"
            nextest = "never"
            ignore = ["docs/**"]
            autostart_daemon = false
            deny_warnings = true

            [build]
            features = ["serde", "cli"]
            target = "aarch64-apple-darwin"
            "#,
        )
        .unwrap();
        assert_eq!(config.budget, 800);
        assert_eq!(config.up_to, Step::Clippy);
        assert_eq!(config.lang, Lang::Es);
        assert_eq!(config.nextest, NextestMode::Never);
        assert_eq!(config.ignore, ["docs/**"]);
        assert!(!config.autostart_daemon);
        assert!(config.deny_warnings);
        assert_eq!(config.build.features, ["serde", "cli"]);
        assert_eq!(config.build.target.as_deref(), Some("aarch64-apple-darwin"));
        assert!(Config::from_toml_str("[build]\nfeaturs = []").is_err());
    }

    #[test]
    fn rejects_unknown_keys_and_values() {
        assert!(Config::from_toml_str("budgte = 3").is_err());
        assert!(Config::from_toml_str("up_to = \"build\"").is_err());
    }

    #[test]
    fn load_missing_file_is_default() {
        let dir = std::env::temp_dir().join("orq-core-config-test-missing");
        assert_eq!(Config::load(&dir).unwrap(), Config::default());
    }

    #[test]
    fn ignore_globs() {
        let config = Config {
            ignore: vec![
                "docs/**".into(),
                "*.md".into(),
                "crates/*/notes?.txt".into(),
            ],
            ..Config::default()
        };
        assert!(config.is_ignored(Path::new("docs/guide/intro.md")));
        assert!(config.is_ignored(Path::new("docs")));
        assert!(config.is_ignored(Path::new("README.md")));
        assert!(!config.is_ignored(Path::new("crates/api/README.md")));
        assert!(config.is_ignored(Path::new("crates/api/notes1.txt")));
        assert!(!config.is_ignored(Path::new("crates/api/src/lib.rs")));
    }
}
