//! Impact analysis: maps changed files to the workspace crates they affect.
//!
//! The workspace graph is read with `cargo metadata --no-deps` (path
//! dependencies between members are enough) and cached in memory per
//! workspace root, so repeated analyses are fast.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};

use cargo_metadata::MetadataCommand;
use orq_core::{ChangeSet, Config, CoreError, CrateId, ImpactAnalyzer, ImpactSet};

/// Errors raised while building the workspace graph.
#[derive(Debug, thiserror::Error)]
enum ImpactError {
    #[error("{0}")]
    Metadata(#[from] cargo_metadata::Error),
    #[error("cannot resolve {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl From<ImpactError> for CoreError {
    fn from(err: ImpactError) -> Self {
        match err {
            ImpactError::Canonicalize { path, source } => CoreError::io(path, source),
            ImpactError::Metadata(e) => CoreError::Metadata(e.to_string()),
        }
    }
}

/// One workspace member.
#[derive(Debug)]
struct Member {
    id: CrateId,
    /// Canonical directory containing the manifest.
    dir: PathBuf,
}

/// Workspace members and their direct dependency edges.
#[derive(Debug)]
struct Graph {
    members: Vec<Member>,
    /// `reverse[i]` = indices of members that depend directly on member `i`.
    reverse: Vec<Vec<usize>>,
}

fn canonical(path: &Path) -> Result<PathBuf, ImpactError> {
    path.canonicalize()
        .map_err(|source| ImpactError::Canonicalize {
            path: path.to_path_buf(),
            source,
        })
}

impl Graph {
    fn load(root: &Path) -> Result<Self, ImpactError> {
        let metadata = MetadataCommand::new()
            .manifest_path(root.join("Cargo.toml"))
            .no_deps()
            .exec()?;
        let packages = metadata.workspace_packages();
        let mut members = Vec::new();
        for pkg in &packages {
            let manifest = PathBuf::from(pkg.manifest_path.as_std_path());
            let dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
            members.push(Member {
                id: CrateId {
                    name: pkg.name.to_string(),
                    manifest_path: manifest,
                },
                dir: canonical(&dir)?,
            });
        }
        members.sort_by(|a, b| a.id.cmp(&b.id));

        let mut reverse = vec![Vec::new(); members.len()];
        for pkg in &packages {
            let Some(me) = members.iter().position(|m| m.id.name == *pkg.name) else {
                continue;
            };
            for dep in &pkg.dependencies {
                let Some(path) = &dep.path else { continue };
                let Ok(dep_dir) = path.as_std_path().canonicalize() else {
                    continue;
                };
                if let Some(target) = members.iter().position(|m| m.dir == dep_dir) {
                    if target != me && !reverse[target].contains(&me) {
                        reverse[target].push(me);
                    }
                }
            }
        }
        Ok(Self { members, reverse })
    }

    /// Deepest member whose directory contains `abs`.
    fn owner(&self, abs: &Path) -> Option<usize> {
        self.members
            .iter()
            .enumerate()
            .filter(|(_, m)| abs.starts_with(&m.dir))
            .max_by_key(|(_, m)| m.dir.components().count())
            .map(|(i, _)| i)
    }

    /// Transitive reverse dependents of `changed`, excluding `changed`.
    fn dependents(&self, changed: &BTreeSet<usize>) -> BTreeSet<usize> {
        let mut seen: BTreeSet<usize> = changed.clone();
        let mut stack: Vec<usize> = changed.iter().copied().collect();
        let mut out = BTreeSet::new();
        while let Some(i) = stack.pop() {
            for &d in &self.reverse[i] {
                if seen.insert(d) {
                    out.insert(d);
                    stack.push(d);
                }
            }
        }
        out
    }
}

/// `true` if a change to `rel` (relative to the root) invalidates the whole
/// workspace.
fn is_full_rebuild_trigger(rel: &Path) -> bool {
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == "build.rs" {
        return true;
    }
    if rel.components().count() == 1 {
        return name == "Cargo.toml" || name == "Cargo.lock" || name.starts_with("rust-toolchain");
    }
    let in_dot_cargo = rel
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d == ".cargo");
    in_dot_cargo && name.starts_with("config")
}

/// Impact analyzer based on `cargo metadata`, with an in-memory cache of the
/// workspace graph.
pub struct CargoImpactAnalyzer {
    config: Config,
    cache: Mutex<HashMap<PathBuf, Arc<Graph>>>,
}

impl CargoImpactAnalyzer {
    /// Creates an analyzer that honours `config.ignore`.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Drops cached metadata for `root` (call when a `Cargo.toml` changes).
    pub fn invalidate(&self, root: &Path) {
        let mut cache = self.lock();
        cache.remove(root);
        if let Ok(canon) = root.canonicalize() {
            cache.remove(&canon);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Arc<Graph>>> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn graph(&self, canon_root: &Path) -> Result<Arc<Graph>, ImpactError> {
        if let Some(g) = self.lock().get(canon_root) {
            return Ok(Arc::clone(g));
        }
        let graph = Arc::new(Graph::load(canon_root)?);
        tracing::debug!(root = %canon_root.display(), crates = graph.members.len(), "loaded cargo metadata");
        self.lock()
            .insert(canon_root.to_path_buf(), Arc::clone(&graph));
        Ok(graph)
    }
}

impl ImpactAnalyzer for CargoImpactAnalyzer {
    fn analyze(&self, root: &Path, changes: &ChangeSet) -> Result<ImpactSet, CoreError> {
        let canon_root = canonical(root)?;
        let graph = self.graph(&canon_root)?;

        let mut full_rebuild = false;
        let mut changed = BTreeSet::new();
        for file in &changes.files {
            if self.config.is_ignored(file) {
                continue;
            }
            if is_full_rebuild_trigger(file) {
                full_rebuild = true;
            }
            if let Some(i) = graph.owner(&canon_root.join(file)) {
                changed.insert(i);
            }
        }
        let dependents = graph.dependents(&changed);
        let ids = |set: &BTreeSet<usize>| -> Vec<CrateId> {
            let mut v: Vec<CrateId> = set.iter().map(|&i| graph.members[i].id.clone()).collect();
            v.sort();
            v
        };
        Ok(ImpactSet {
            changed: ids(&changed),
            dependents: ids(&dependents),
            full_rebuild,
        })
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<Vec<u8>, CoreError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| CoreError::Process {
            command: format!("git {}", args.join(" ")),
            message: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(CoreError::Git(format!(
            "`git {}` exited with {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// `git diff --name-only HEAD` ∪ untracked files, relative to `root`, sorted
/// and deduplicated.
pub fn detect_changes(root: &Path) -> Result<ChangeSet, CoreError> {
    let tracked = run_git(root, &["diff", "--name-only", "--relative", "-z", "HEAD"])?;
    let untracked = run_git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let files: BTreeSet<PathBuf> = tracked
        .split(|b| *b == 0)
        .chain(untracked.split(|b| *b == 0))
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(String::from_utf8_lossy(s).into_owned()))
        .filter(|p| !p.components().any(|c| matches!(c, Component::ParentDir)))
        .collect();
    Ok(ChangeSet {
        files: files.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }

    fn changes(files: &[&str]) -> ChangeSet {
        ChangeSet {
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    fn names(ids: &[CrateId]) -> Vec<&str> {
        ids.iter().map(|c| c.name.as_str()).collect()
    }

    fn analyze(fixture_name: &str, files: &[&str]) -> ImpactSet {
        CargoImpactAnalyzer::new(Config::default())
            .analyze(&fixture(fixture_name), &changes(files))
            .unwrap()
    }

    #[test]
    fn leaf_only_affects_leaf() {
        let i = analyze("multi", &["crates/leaf/src/lib.rs"]);
        assert_eq!(names(&i.changed), ["leaf"]);
        assert!(i.dependents.is_empty());
        assert!(!i.full_rebuild);
    }

    #[test]
    fn base_affects_dependents() {
        let i = analyze("multi", &["crates/base/src/lib.rs"]);
        assert_eq!(names(&i.changed), ["base"]);
        assert_eq!(names(&i.dependents), ["app", "mid"]);
        let affected = i.affected();
        assert_eq!(names(&affected), ["app", "base", "mid"]);
    }

    #[test]
    fn dependents_exclude_changed() {
        let i = analyze(
            "multi",
            &["crates/base/src/lib.rs", "crates/mid/src/lib.rs"],
        );
        assert_eq!(names(&i.changed), ["base", "mid"]);
        assert_eq!(names(&i.dependents), ["app"]);
    }

    #[test]
    fn cargo_lock_is_full_rebuild() {
        let i = analyze("multi", &["Cargo.lock"]);
        assert!(i.full_rebuild);
        assert!(i.changed.is_empty());
    }

    #[test]
    fn workspace_wide_inputs_are_full_rebuild() {
        for f in [
            "Cargo.toml",
            "rust-toolchain.toml",
            "rust-toolchain",
            ".cargo/config.toml",
            ".cargo/config",
        ] {
            assert!(analyze("multi", &[f]).full_rebuild, "{f}");
        }
    }

    #[test]
    fn member_manifest_is_not_full_rebuild() {
        let i = analyze("multi", &["crates/mid/Cargo.toml"]);
        assert!(!i.full_rebuild);
        assert_eq!(names(&i.changed), ["mid"]);
        assert_eq!(names(&i.dependents), ["app"]);
    }

    #[test]
    fn build_rs_is_full_rebuild() {
        let i = analyze("build-rs", &["build.rs"]);
        assert!(i.full_rebuild);
        assert_eq!(i.changed.len(), 1);
    }

    #[test]
    fn single_crate_maps_nested_files() {
        let i = analyze("single", &["src/shapes/mod.rs"]);
        assert_eq!(i.changed.len(), 1);
        assert!(i.dependents.is_empty());
        assert!(!i.full_rebuild);
    }

    #[test]
    fn outside_files_are_ignored() {
        let i = analyze("multi", &["docs/guide.md", "README.md"]);
        assert!(i.is_empty());
    }

    #[test]
    fn config_ignore_applies() {
        let config = Config {
            ignore: vec!["crates/leaf/**".into()],
            ..Config::default()
        };
        let i = CargoImpactAnalyzer::new(config)
            .analyze(&fixture("multi"), &changes(&["crates/leaf/src/lib.rs"]))
            .unwrap();
        assert!(i.is_empty());
    }

    #[test]
    fn output_is_sorted() {
        let i = analyze(
            "multi",
            &[
                "crates/mid/src/lib.rs",
                "crates/base/src/lib.rs",
                "crates/leaf/src/lib.rs",
            ],
        );
        assert_eq!(names(&i.changed), ["base", "leaf", "mid"]);
    }

    #[test]
    fn second_analyze_is_fast() {
        let analyzer = CargoImpactAnalyzer::new(Config::default());
        let root = fixture("multi");
        let c = changes(&["crates/base/src/lib.rs"]);
        analyzer.analyze(&root, &c).unwrap();
        let start = Instant::now();
        analyzer.analyze(&root, &c).unwrap();
        assert!(start.elapsed().as_millis() < 300, "{:?}", start.elapsed());
    }

    #[test]
    fn invalidate_forces_reload() {
        let analyzer = CargoImpactAnalyzer::new(Config::default());
        let root = fixture("multi");
        analyzer.analyze(&root, &changes(&[])).unwrap();
        assert_eq!(analyzer.lock().len(), 1);
        analyzer.invalidate(&root);
        assert!(analyzer.lock().is_empty());
    }

    fn git(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.file_name() == "target" {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    #[test]
    fn detect_changes_sees_modified_and_untracked() {
        if !git(Path::new("."), &["--version"]) {
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        copy_dir(&fixture("multi"), root);
        assert!(git(root, &["init", "-q"]));
        assert!(git(root, &["config", "user.name", "t"]));
        assert!(git(root, &["config", "user.email", "t@example.com"]));
        assert!(git(root, &["config", "commit.gpgsign", "false"]));
        assert!(git(root, &["add", "-A"]));
        assert!(git(root, &["commit", "-q", "-m", "init"]));

        assert!(detect_changes(root).unwrap().files.is_empty());

        let base = root.join("crates/base/src/lib.rs");
        let mut src = std::fs::read_to_string(&base).unwrap();
        src.push_str("\n// touched\n");
        std::fs::write(&base, src).unwrap();
        std::fs::write(root.join("crates/leaf/src/new.rs"), "// new\n").unwrap();

        let set = detect_changes(root).unwrap();
        assert_eq!(
            set.files,
            [
                PathBuf::from("crates/base/src/lib.rs"),
                PathBuf::from("crates/leaf/src/new.rs")
            ]
        );
        let impact = CargoImpactAnalyzer::new(Config::default())
            .analyze(root, &set)
            .unwrap();
        assert_eq!(names(&impact.changed), ["base", "leaf"]);
        assert_eq!(names(&impact.dependents), ["app", "mid"]);
    }

    #[test]
    fn detect_changes_fails_outside_git() {
        if !git(Path::new("."), &["--version"]) {
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        // A temp dir is normally not inside a repo; skip if it happens to be.
        if git(tmp.path(), &["rev-parse", "--git-dir"]) {
            return;
        }
        assert!(detect_changes(tmp.path()).is_err());
    }
}
