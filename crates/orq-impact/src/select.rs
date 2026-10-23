//! Best-effort selection of the test targets a change needs.
//!
//! Impact analysis works per crate. For tests that is coarse: editing
//! `tests/foo.rs` cannot change the result of the library's unit tests, of the
//! other integration tests or of any dependent. [`TestSelection`] narrows the
//! Test step down to what can be affected, and errs on the side of running
//! more: anything it cannot classify counts as a library change.
//!
//! Rules, per changed crate (files relative to its directory):
//!
//! - A file that is an integration test target's root (`tests/foo.rs`, a
//!   `[[test]] path`, or anything under the directory of a `main.rs` root)
//!   selects only that target.
//! - Any other file under `tests/` is shared support code (`tests/common/`,
//!   fixtures): all the crate's integration targets run, nothing else.
//! - Files under `benches/` and `examples/` run no tests; the compile steps
//!   already cover them.
//! - Everything else (`src/`, `build.rs`, `Cargo.toml`, data read by
//!   `include_str!`...) is a library change: all tests of the crate (lib,
//!   bins, integration, doc) and of its transitive dependents.
//! - A workspace-wide trigger (`ImpactSet::full_rebuild`) selects all tests
//!   of every crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use orq_core::{CrateId, ImpactSet};

use crate::{Graph, Member};

/// An integration test target of a workspace member.
#[derive(Debug)]
pub(crate) struct IntegrationTarget {
    pub(crate) name: String,
    /// Canonical path of the target's root file.
    pub(crate) src: PathBuf,
}

/// What to run for the packages of a [`TestRun`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestScope {
    /// Every test of the packages: lib, bins, integration and doc tests.
    All,
    /// Only these integration test targets (`--test <name>`).
    Integration(Vec<String>),
}

/// One cargo test invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    /// The packages to test. Several packages only for [`TestScope::All`].
    pub packages: Vec<CrateId>,
    /// What to run in them.
    pub scope: TestScope,
}

impl TestRun {
    /// Target-selection flags to append after the `-p` arguments:
    /// empty for [`TestScope::All`], `--test <name>` per target otherwise.
    pub fn target_args(&self) -> Vec<String> {
        match &self.scope {
            TestScope::All => Vec::new(),
            TestScope::Integration(names) => names
                .iter()
                .flat_map(|n| ["--test".to_string(), n.clone()])
                .collect(),
        }
    }
}

/// The test invocations a change needs. Empty `runs` means no test can be
/// affected (for example only a bench changed): the Test step passes without
/// running anything. This is different from "unknown", which the selection
/// never returns: unclassifiable input selects everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestSelection {
    /// Invocations, [`TestScope::All`] first, then per-package ones sorted by
    /// package name.
    pub runs: Vec<TestRun>,
}

impl TestSelection {
    /// `true` if no test needs to run.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// `true` if every selected run is [`TestScope::All`], i.e. the selection
    /// is no finer than per-crate.
    pub fn is_whole_crates(&self) -> bool {
        self.runs.iter().all(|r| r.scope == TestScope::All)
    }
}

/// Selects the tests for `impact` reading the workspace at `root` with a
/// fresh `cargo metadata`. Prefer
/// [`CargoImpactAnalyzer::select_tests`](crate::CargoImpactAnalyzer::select_tests),
/// which reuses the cached graph.
pub fn select_tests(root: &Path, impact: &ImpactSet) -> Result<TestSelection, orq_core::CoreError> {
    let canon_root = crate::canonical(root)?;
    let graph = Graph::load(&canon_root)?;
    Ok(select(&graph, &canon_root, impact))
}

/// How one changed file affects the tests of its crate.
#[derive(Debug, PartialEq, Eq)]
enum Effect<'a> {
    Library,
    Target(&'a str),
    AllIntegration,
    Nothing,
}

fn classify<'a>(member: &'a Member, abs: &Path) -> Effect<'a> {
    for t in &member.integration {
        if abs == t.src {
            return Effect::Target(&t.name);
        }
        let is_main = t.src.file_name().is_some_and(|n| n == "main.rs");
        if let (true, Some(dir)) = (is_main, t.src.parent()) {
            if dir != member.dir.join("src") && dir != member.dir && abs.starts_with(dir) {
                return Effect::Target(&t.name);
            }
        }
    }
    let Ok(rel) = abs.strip_prefix(&member.dir) else {
        return Effect::Library;
    };
    match rel.components().next().and_then(|c| c.as_os_str().to_str()) {
        Some("tests") => Effect::AllIntegration,
        Some("benches" | "examples") => Effect::Nothing,
        _ => Effect::Library,
    }
}

pub(crate) fn select(graph: &Graph, canon_root: &Path, impact: &ImpactSet) -> TestSelection {
    let id = |i: usize| graph.members[i].id.clone();
    if impact.full_rebuild {
        return TestSelection {
            runs: vec![TestRun {
                packages: (0..graph.members.len()).map(id).collect(),
                scope: TestScope::All,
            }],
        };
    }

    let changed: BTreeSet<usize> = impact
        .changed
        .iter()
        .filter_map(|c| graph.members.iter().position(|m| m.id == *c))
        .collect();

    let mut library: BTreeSet<usize> = BTreeSet::new();
    let mut targets: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for &i in &changed {
        let member = &graph.members[i];
        let files: Vec<PathBuf> = impact
            .changed_files
            .iter()
            .map(|f| canon_root.join(f))
            .filter(|abs| graph.owner(abs) == Some(i))
            .collect();
        if files.is_empty() {
            // Changed crate without a file that says why: be conservative.
            library.insert(i);
            continue;
        }
        for abs in &files {
            match classify(member, abs) {
                Effect::Library => {
                    library.insert(i);
                }
                Effect::Target(name) => {
                    targets.entry(i).or_default().insert(name.to_string());
                }
                Effect::AllIntegration => {
                    targets
                        .entry(i)
                        .or_default()
                        .extend(member.integration.iter().map(|t| t.name.clone()));
                }
                Effect::Nothing => {}
            }
        }
    }

    let mut whole: BTreeSet<usize> = library.clone();
    whole.extend(graph.dependents(&library));

    let mut runs = Vec::new();
    if !whole.is_empty() {
        runs.push(TestRun {
            packages: whole.iter().copied().map(id).collect(),
            scope: TestScope::All,
        });
    }
    for (i, names) in targets {
        if whole.contains(&i) || names.is_empty() {
            continue;
        }
        runs.push(TestRun {
            packages: vec![id(i)],
            scope: TestScope::Integration(names.into_iter().collect()),
        });
    }
    TestSelection { runs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CargoImpactAnalyzer;
    use orq_core::{ChangeSet, Config, ImpactAnalyzer};

    /// `base` <- `mid`, plus independent `leaf`. `base` has the integration
    /// targets `alpha` (tests/alpha.rs) and `beta` (tests/beta/main.rs) and
    /// shared code in `tests/common/mod.rs`.
    fn workspace() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let w = |rel: &str, body: &str| {
            let p = tmp.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        w(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/base\", \"crates/mid\", \"crates/leaf\"]\nresolver = \"2\"\n",
        );
        let manifest = |name: &str, dep: &str| {
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 [dependencies]\n{dep}\n"
            )
        };
        w("crates/base/Cargo.toml", &manifest("base", ""));
        w(
            "crates/mid/Cargo.toml",
            &manifest("mid", "base = { path = \"../base\" }"),
        );
        w("crates/leaf/Cargo.toml", &manifest("leaf", ""));
        for c in ["base", "mid", "leaf"] {
            w(&format!("crates/{c}/src/lib.rs"), "");
        }
        w("crates/base/tests/alpha.rs", "");
        w("crates/base/tests/beta/main.rs", "");
        w("crates/base/tests/beta/helper.rs", "");
        w("crates/base/tests/common/mod.rs", "");
        w("crates/base/benches/b.rs", "");
        w("crates/mid/tests/gamma.rs", "");
        tmp
    }

    fn select_for(root: &Path, files: &[&str]) -> TestSelection {
        let analyzer = CargoImpactAnalyzer::new(Config::default());
        let changes = ChangeSet {
            files: files.iter().map(PathBuf::from).collect(),
        };
        let impact = analyzer.analyze(root, &changes).unwrap();
        analyzer.select_tests(root, &impact).unwrap()
    }

    fn describe(sel: &TestSelection) -> Vec<String> {
        sel.runs
            .iter()
            .map(|r| {
                let pkgs: Vec<&str> = r.packages.iter().map(|p| p.name.as_str()).collect();
                match &r.scope {
                    TestScope::All => format!("all:{}", pkgs.join(",")),
                    TestScope::Integration(n) => {
                        format!("{}:--test {}", pkgs.join(","), n.join(","))
                    }
                }
            })
            .collect()
    }

    #[test]
    fn single_integration_file_selects_only_its_target() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/base/tests/alpha.rs"]);
        assert_eq!(describe(&sel), ["base:--test alpha"]);
        assert_eq!(
            sel.runs[0].target_args(),
            ["--test".to_string(), "alpha".to_string()]
        );
        assert!(!sel.is_whole_crates());
    }

    #[test]
    fn file_in_a_main_rs_target_directory_selects_that_target() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/base/tests/beta/helper.rs"]);
        assert_eq!(describe(&sel), ["base:--test beta"]);
        let sel = select_for(tmp.path(), &["crates/base/tests/beta/main.rs"]);
        assert_eq!(describe(&sel), ["base:--test beta"]);
    }

    #[test]
    fn shared_test_support_selects_all_integration_targets_only() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/base/tests/common/mod.rs"]);
        assert_eq!(describe(&sel), ["base:--test alpha,beta"]);
    }

    #[test]
    fn integration_tests_in_two_crates_make_two_runs() {
        let tmp = workspace();
        let sel = select_for(
            tmp.path(),
            &["crates/mid/tests/gamma.rs", "crates/base/tests/alpha.rs"],
        );
        assert_eq!(describe(&sel), ["base:--test alpha", "mid:--test gamma"]);
    }

    #[test]
    fn library_change_selects_whole_crate_and_dependents() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/base/src/lib.rs"]);
        assert_eq!(describe(&sel), ["all:base,mid"]);
        assert!(sel.is_whole_crates());
        let sel = select_for(tmp.path(), &["crates/leaf/src/lib.rs"]);
        assert_eq!(describe(&sel), ["all:leaf"]);
    }

    #[test]
    fn library_change_absorbs_integration_change_of_same_crate() {
        let tmp = workspace();
        let sel = select_for(
            tmp.path(),
            &["crates/base/src/lib.rs", "crates/base/tests/alpha.rs"],
        );
        assert_eq!(describe(&sel), ["all:base,mid"]);
    }

    #[test]
    fn integration_change_in_a_dependent_stays_narrow_beside_a_library_change() {
        let tmp = workspace();
        let sel = select_for(
            tmp.path(),
            &["crates/leaf/src/lib.rs", "crates/base/tests/alpha.rs"],
        );
        assert_eq!(describe(&sel), ["all:leaf", "base:--test alpha"]);
    }

    #[test]
    fn bench_only_change_runs_no_tests() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/base/benches/b.rs"]);
        assert!(sel.is_empty());
        assert!(sel.is_whole_crates());
    }

    #[test]
    fn manifest_and_unknown_files_count_as_library_changes() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["crates/mid/Cargo.toml"]);
        assert_eq!(describe(&sel), ["all:mid"]);
        let sel = select_for(tmp.path(), &["crates/mid/README.md"]);
        assert_eq!(describe(&sel), ["all:mid"]);
    }

    #[test]
    fn full_rebuild_selects_everything() {
        let tmp = workspace();
        let sel = select_for(tmp.path(), &["Cargo.lock"]);
        assert_eq!(describe(&sel), ["all:base,leaf,mid"]);
    }

    #[test]
    fn changed_crate_without_files_selects_the_whole_crate() {
        let tmp = workspace();
        let analyzer = CargoImpactAnalyzer::new(Config::default());
        let mut impact = analyzer
            .analyze(
                tmp.path(),
                &ChangeSet {
                    files: vec!["crates/base/tests/alpha.rs".into()],
                },
            )
            .unwrap();
        impact.changed_files.clear();
        let sel = analyzer.select_tests(tmp.path(), &impact).unwrap();
        assert_eq!(describe(&sel), ["all:base,mid"]);
    }

    #[test]
    fn free_function_matches_the_method() {
        let tmp = workspace();
        let analyzer = CargoImpactAnalyzer::new(Config::default());
        let impact = analyzer
            .analyze(
                tmp.path(),
                &ChangeSet {
                    files: vec!["crates/base/tests/alpha.rs".into()],
                },
            )
            .unwrap();
        assert_eq!(
            select_tests(tmp.path(), &impact).unwrap(),
            analyzer.select_tests(tmp.path(), &impact).unwrap()
        );
    }
}
