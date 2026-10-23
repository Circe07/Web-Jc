//! Maps cargo output (manifest paths, package ids, source files) to the
//! [`CrateId`]s of the impact set.

use std::path::{Path, PathBuf};

use orq_core::CrateId;

/// Lookup table over the crates being verified.
pub(crate) struct CrateIndex {
    root: PathBuf,
    canon_root: PathBuf,
    /// `(crate, canonical crate directory)`.
    crates: Vec<(CrateId, PathBuf)>,
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl CrateIndex {
    pub(crate) fn new(root: &Path, crates: Vec<CrateId>) -> Self {
        let crates = crates
            .into_iter()
            .map(|c| {
                let dir = c.manifest_path.parent().map(canon).unwrap_or_default();
                (c, dir)
            })
            .collect();
        Self {
            root: root.to_path_buf(),
            canon_root: canon(root),
            crates,
        }
    }

    /// The workspace root.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Makes `path` relative to the workspace root when it lies inside it.
    pub(crate) fn relative(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            for base in [&self.root, &self.canon_root] {
                if let Ok(rel) = path.strip_prefix(base) {
                    return rel.to_path_buf();
                }
            }
            if let Ok(rel) = canon(path).strip_prefix(&self.canon_root) {
                return rel.to_path_buf();
            }
        }
        path.to_path_buf()
    }

    /// Resolves the crate a cargo message belongs to.
    pub(crate) fn resolve(
        &self,
        manifest_path: Option<&Path>,
        package_id: Option<&str>,
    ) -> Option<CrateId> {
        if let Some(manifest) = manifest_path {
            let canon_manifest = canon(manifest);
            if let Some((c, _)) = self.crates.iter().find(|(c, _)| {
                c.manifest_path == manifest || canon(&c.manifest_path) == canon_manifest
            }) {
                return Some(c.clone());
            }
        }
        let name = package_id.and_then(package_name);
        if let Some(name) = &name {
            if let Some((c, _)) = self.crates.iter().find(|(c, _)| &c.name == name) {
                return Some(c.clone());
            }
        }
        // Not in the impact set (e.g. a dependency inside the workspace):
        // describe it from what cargo told us.
        match (name, manifest_path) {
            (Some(name), Some(manifest)) => Some(CrateId {
                name,
                manifest_path: manifest.to_path_buf(),
            }),
            _ => None,
        }
    }

    /// Crate owning `path` (absolute or relative to the root): longest
    /// matching crate directory.
    pub(crate) fn for_path(&self, path: &Path) -> Option<CrateId> {
        let abs = if path.is_absolute() {
            canon(path)
        } else {
            self.canon_root.join(path)
        };
        self.crates
            .iter()
            .filter(|(_, dir)| abs.starts_with(dir))
            .max_by_key(|(_, dir)| dir.components().count())
            .map(|(c, _)| c.clone())
    }

    /// Crate whose name matches a binary/doc-test name (`-` and `_` equal).
    pub(crate) fn for_name(&self, name: &str) -> Option<CrateId> {
        let norm = |s: &str| s.replace('-', "_");
        let name = norm(name);
        self.crates
            .iter()
            .find(|(c, _)| norm(&c.name) == name)
            .map(|(c, _)| c.clone())
    }

    /// Best guess when nothing more specific is known.
    pub(crate) fn fallback(&self) -> CrateId {
        match self.crates.first() {
            Some((c, _)) => c.clone(),
            None => CrateId {
                name: self
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                manifest_path: self.root.join("Cargo.toml"),
            },
        }
    }
}

/// Extracts the package name from a cargo `package_id`.
///
/// Handles `path+file:///x/name#0.1.0`, `path+file:///x#name@0.1.0` and
/// `registry+https://...#name@1.0.0`.
pub(crate) fn package_name(package_id: &str) -> Option<String> {
    let (url, fragment) = package_id.split_once('#')?;
    if let Some((name, _)) = fragment.split_once('@') {
        return Some(name.to_string());
    }
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty()).then(|| last.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names() {
        assert_eq!(
            package_name("path+file:///a/b/broken-type#0.1.0").as_deref(),
            Some("broken-type")
        );
        assert_eq!(
            package_name("path+file:///a/b#app@0.1.0").as_deref(),
            Some("app")
        );
        assert_eq!(
            package_name("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0")
                .as_deref(),
            Some("serde")
        );
    }

    #[test]
    fn path_resolution() {
        let krate = |n: &str| CrateId {
            name: n.into(),
            manifest_path: PathBuf::from(format!("/ws/crates/{n}/Cargo.toml")),
        };
        let index = CrateIndex::new(Path::new("/ws"), vec![krate("a"), krate("b")]);
        assert_eq!(
            index
                .for_path(Path::new("crates/b/src/lib.rs"))
                .map(|c| c.name),
            Some("b".into())
        );
        assert_eq!(
            index.relative(Path::new("/ws/crates/a/src/lib.rs")),
            PathBuf::from("crates/a/src/lib.rs")
        );
        assert_eq!(index.for_name("a").map(|c| c.name), Some("a".into()));
    }
}
