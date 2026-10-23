//! Cargo target selection for the compile steps (`check` and `clippy`).
//!
//! `--all-targets` also builds benches and examples, which is slow and can
//! fail on code that is irrelevant to the change (benches that need nightly).
//! With [`Targets::Auto`] we only pass `--lib --bins --tests`, plus
//! `--benches` / `--examples` when a changed file lives in the matching
//! directory of an affected crate.
//!
//! `--lib` makes cargo fail with "no library targets found" when none of the
//! selected packages has a library, so the flags are derived from
//! `cargo metadata`. With several packages cargo accepts `--lib` as long as
//! at least one of them has a library.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cargo_metadata::MetadataCommand;
use orq_core::{ImpactSet, Targets};

/// What a package offers and where it lives.
#[derive(Debug)]
struct PackageTargets {
    name: String,
    /// Package directory relative to the workspace root.
    dir: PathBuf,
    has_lib: bool,
    has_bin: bool,
    has_bench: bool,
    has_example: bool,
}

/// Target flags for `cargo check` / `cargo clippy`.
///
/// Falls back to `--all-targets` when the metadata cannot be read, so the
/// result is always something cargo accepts.
pub(crate) async fn compile_target_args(
    cargo: OsString,
    root: &Path,
    impact: &ImpactSet,
    targets: Targets,
) -> Vec<String> {
    if targets == Targets::All {
        return vec!["--all-targets".to_string()];
    }
    let root = root.to_path_buf();
    let impact = impact.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let packages = load_packages(&cargo, &root)?;
        Some(auto_args(&packages, &impact))
    })
    .await;
    match loaded {
        Ok(Some(args)) => args,
        _ => {
            tracing::debug!("cargo metadata unavailable, falling back to --all-targets");
            vec!["--all-targets".to_string()]
        }
    }
}

fn load_packages(cargo: &OsString, root: &Path) -> Option<Vec<PackageTargets>> {
    let metadata = MetadataCommand::new()
        .cargo_path(cargo)
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .ok()?;
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let packages = metadata
        .workspace_packages()
        .into_iter()
        .map(|pkg| {
            let manifest = PathBuf::from(pkg.manifest_path.as_std_path());
            let dir = manifest.parent().map(Path::to_path_buf).unwrap_or_default();
            let dir = dir.canonicalize().unwrap_or(dir);
            let kinds: Vec<String> = pkg
                .targets
                .iter()
                .flat_map(|t| t.kind.iter().map(ToString::to_string))
                .collect();
            let any = |wanted: &[&str]| kinds.iter().any(|k| wanted.contains(&k.as_str()));
            PackageTargets {
                name: pkg.name.to_string(),
                dir: dir
                    .strip_prefix(&canon_root)
                    .map(Path::to_path_buf)
                    .unwrap_or(dir),
                has_lib: any(&["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"]),
                has_bin: any(&["bin"]),
                has_bench: any(&["bench"]),
                has_example: any(&["example"]),
            }
        })
        .collect();
    Some(packages)
}

/// The flags for `Targets::Auto` given the workspace packages.
fn auto_args(packages: &[PackageTargets], impact: &ImpactSet) -> Vec<String> {
    let selected: HashSet<String> = if impact.full_rebuild {
        packages.iter().map(|p| p.name.clone()).collect()
    } else {
        impact.affected().into_iter().map(|c| c.name).collect()
    };
    let selected: Vec<&PackageTargets> = packages
        .iter()
        .filter(|p| selected.contains(&p.name))
        .collect();
    if selected.is_empty() {
        // Unknown packages: let cargo report the problem with all targets.
        return vec!["--all-targets".to_string()];
    }
    let changed_in = |p: &PackageTargets, sub: &str| {
        let prefix = p.dir.join(sub);
        impact.changed_files.iter().any(|f| f.starts_with(&prefix))
    };

    let mut args = Vec::new();
    if selected.iter().any(|p| p.has_lib) {
        args.push("--lib".to_string());
    }
    if selected.iter().any(|p| p.has_bin) {
        args.push("--bins".to_string());
    }
    args.push("--tests".to_string());
    if selected
        .iter()
        .any(|p| p.has_bench && changed_in(p, "benches"))
    {
        args.push("--benches".to_string());
    }
    if selected
        .iter()
        .any(|p| p.has_example && changed_in(p, "examples"))
    {
        args.push("--examples".to_string());
    }
    args
}

#[cfg(test)]
mod tests {
    use orq_core::CrateId;

    use super::*;

    fn pkg(name: &str, lib: bool, bin: bool) -> PackageTargets {
        PackageTargets {
            name: name.into(),
            dir: PathBuf::from("crates").join(name),
            has_lib: lib,
            has_bin: bin,
            has_bench: true,
            has_example: true,
        }
    }

    fn impact(names: &[&str], files: &[&str]) -> ImpactSet {
        ImpactSet {
            changed: names
                .iter()
                .map(|n| CrateId {
                    name: (*n).into(),
                    manifest_path: PathBuf::from(format!("/ws/crates/{n}/Cargo.toml")),
                })
                .collect(),
            changed_files: files.iter().map(PathBuf::from).collect(),
            ..ImpactSet::default()
        }
    }

    #[test]
    fn lib_and_bins_follow_the_selected_packages() {
        let pkgs = [pkg("a", true, false), pkg("b", false, true)];
        assert_eq!(auto_args(&pkgs, &impact(&["a"], &[])), ["--lib", "--tests"]);
        assert_eq!(
            auto_args(&pkgs, &impact(&["b"], &[])),
            ["--bins", "--tests"]
        );
        assert_eq!(
            auto_args(&pkgs, &impact(&["a", "b"], &[])),
            ["--lib", "--bins", "--tests"]
        );
    }

    #[test]
    fn benches_and_examples_only_when_changed() {
        let pkgs = [pkg("a", true, false), pkg("b", true, false)];
        let files = ["crates/a/benches/x.rs", "crates/b/examples/e.rs"];
        assert_eq!(
            auto_args(&pkgs, &impact(&["a", "b"], &files)),
            ["--lib", "--tests", "--benches", "--examples"]
        );
        // A bench of an unaffected crate does not count.
        assert_eq!(
            auto_args(&pkgs, &impact(&["b"], &["crates/a/benches/x.rs"])),
            ["--lib", "--tests"]
        );
    }

    #[test]
    fn unknown_packages_fall_back_to_all_targets() {
        let pkgs = [pkg("a", true, false)];
        assert_eq!(auto_args(&pkgs, &impact(&["zzz"], &[])), ["--all-targets"]);
    }
}
