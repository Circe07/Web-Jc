//! Per-verification statistics, appended as JSON lines to
//! `target/orq/stats.jsonl` (read by `orq stats`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{root, CompactReport, CoreError, Outcome, Step, VerifyResult};

/// One line of `stats.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsRecord {
    /// Unix time in milliseconds when the verification finished.
    pub timestamp_ms: u64,
    /// Wall-clock duration of the verification ([`VerifyResult::total_ms`]).
    pub duration_ms: u64,
    /// Served from the result cache.
    pub cache_hit: bool,
    /// Served by the daemon rather than in-process.
    #[serde(default)]
    pub via_daemon: bool,
    /// Estimated tokens of the report sent to the agent.
    pub tokens: usize,
    /// Affected crates (`["*"]` on a full rebuild).
    pub crates: Vec<String>,
    /// `"pass"` or `"fail"`.
    pub outcome: String,
    /// Last step requested.
    pub up_to: Step,
    /// Where it came from: `"cli"`, `"mcp"`...
    #[serde(default)]
    pub source: String,
}

impl StatsRecord {
    /// Builds the record for one verification.
    pub fn new(
        result: &VerifyResult,
        report: &CompactReport,
        up_to: Step,
        via_daemon: bool,
        source: &str,
    ) -> Self {
        let crates = if result.impact.full_rebuild {
            vec!["*".to_string()]
        } else {
            result
                .impact
                .affected()
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        Self {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            duration_ms: result.total_ms,
            cache_hit: result.cache_hit,
            via_daemon,
            tokens: report.estimated_tokens,
            crates,
            outcome: match result.outcome {
                Outcome::Pass => "pass".into(),
                Outcome::Fail { .. } => "fail".into(),
            },
            up_to,
            source: source.into(),
        }
    }
}

/// `<target>/orq/stats.jsonl` for the workspace at `root`.
pub fn stats_path(root: &Path) -> PathBuf {
    root::target_dir(root).join("orq").join("stats.jsonl")
}

/// Appends `record` to [`stats_path`], creating directories as needed.
pub fn append(root: &Path, record: &StatsRecord) -> Result<(), CoreError> {
    let path = stats_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
    }
    let mut line = serde_json::to_string(record).map_err(|e| CoreError::Parse(e.to_string()))?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| CoreError::io(path, e))
}

/// Reads every parseable record of [`stats_path`] (missing file = empty).
pub fn read_all(root: &Path) -> Result<Vec<StatsRecord>, CoreError> {
    let path = stats_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(CoreError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CrateId, ImpactSet};

    #[test]
    fn append_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let result = VerifyResult {
            outcome: Outcome::Pass,
            steps: Vec::new(),
            diagnostics: Vec::new(),
            test_failures: Vec::new(),
            impact: ImpactSet {
                changed: vec![CrateId {
                    name: "leaf".into(),
                    manifest_path: PathBuf::new(),
                }],
                ..ImpactSet::default()
            },
            cache_hit: true,
            total_ms: 12,
        };
        let report = CompactReport {
            text: "PASS".into(),
            json: serde_json::Value::Null,
            estimated_tokens: 3,
            truncated: false,
        };
        let record = StatsRecord::new(&result, &report, Step::Test, false, "cli");
        append(dir.path(), &record).unwrap();
        append(dir.path(), &record).unwrap();
        let all = read_all(dir.path()).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].crates, ["leaf"]);
        assert_eq!(all[0].outcome, "pass");
        assert!(all[0].cache_hit);
        assert_eq!(all[0].tokens, 3);
    }
}
