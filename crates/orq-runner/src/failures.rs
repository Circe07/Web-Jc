//! Memory of the tests that failed in the last run, so the next Test step can
//! run them first and fail fast.
//!
//! Stored per workspace in `<target dir>/orq/last_failures.json`. Every
//! problem reading or writing the file is ignored: it is only an
//! optimization.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use orq_core::TestFailure;
use serde::{Deserialize, Serialize};

/// One remembered failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recorded {
    /// Package name.
    #[serde(rename = "crate")]
    pub(crate) krate: String,
    /// Test name as printed by libtest / nextest.
    pub(crate) name: String,
}

/// On-disk format. `root` guards against a target directory shared by
/// several workspaces (`CARGO_TARGET_DIR`): entries of another workspace are
/// ignored.
#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    root: String,
    failures: Vec<Recorded>,
}

fn root_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn file_path(root: &Path) -> PathBuf {
    orq_core::root::target_dir(root)
        .join("orq")
        .join("last_failures.json")
}

/// Failures remembered for this workspace (empty when unknown).
pub(crate) fn load(root: &Path) -> Vec<Recorded> {
    std::fs::read_to_string(file_path(root))
        .ok()
        .and_then(|text| serde_json::from_str::<FileFormat>(&text).ok())
        .filter(|f| f.root == root_key(root))
        .map(|f| f.failures)
        .unwrap_or_default()
}

/// Replaces the remembered failures for the crates in `scope` (`None` = all
/// crates) with `new`, keeping the entries of the other crates.
pub(crate) fn save(root: &Path, scope: Option<&HashSet<String>>, new: Vec<Recorded>) {
    let mut failures: Vec<Recorded> = load(root)
        .into_iter()
        .filter(|r| scope.is_some_and(|s| !s.contains(&r.krate)))
        .collect();
    failures.extend(new);
    failures.sort_by(|a, b| (&a.krate, &a.name).cmp(&(&b.krate, &b.name)));
    failures.dedup();
    let path = file_path(root);
    let result = (|| -> std::io::Result<()> {
        if failures.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&FileFormat {
            root: root_key(root),
            failures,
        })
        .map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(err) = result {
        tracing::debug!(%err, path = %path.display(), "could not save last_failures.json");
    }
}

/// Converts run results to records.
pub(crate) fn record(failures: &[TestFailure]) -> Vec<Recorded> {
    failures
        .iter()
        .map(|f| Recorded {
            krate: f.crate_id.name.clone(),
            name: f.name.clone(),
        })
        .collect()
}

/// `true` if `name` can be passed safely as an exact filter (plain Rust
/// paths; doc-test names with spaces and parentheses are left to the full
/// run).
pub(crate) fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names() {
        assert!(is_plain_name("tests::adds_wrongly"));
        assert!(!is_plain_name("src/lib.rs - foo (line 3)"));
        assert!(!is_plain_name(""));
    }

    #[test]
    fn save_replaces_only_the_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rec = |k: &str, n: &str| Recorded {
            krate: k.into(),
            name: n.into(),
        };
        save(dir.path(), None, vec![rec("a", "t1"), rec("b", "t2")]);
        let scope: HashSet<String> = ["a".to_string()].into();
        save(dir.path(), Some(&scope), vec![rec("a", "t3")]);
        assert_eq!(load(dir.path()), [rec("a", "t3"), rec("b", "t2")]);
        save(dir.path(), None, Vec::new());
        assert!(load(dir.path()).is_empty());
    }
}
