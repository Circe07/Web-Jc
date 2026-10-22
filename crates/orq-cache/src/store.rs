//! The redb-backed [`ResultCache`] implementation.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use orq_core::{CacheKey, CoreError, ImpactSet, ResultCache, Step, VerifyResult, ORQ_VERSION};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::inputs::{collect_files, relevant_env, GLOBAL_FILES};

/// Serialized `VerifyResult`s by key.
const DATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("results");
/// Last-access tick by key (monotonic counter, for LRU).
const ACCESS: TableDefinition<&[u8], u64> = TableDefinition::new("access");
/// Single-row table holding the last issued tick.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const TICK_ROW: &str = "tick";

/// Default maximum number of stored entries.
const DEFAULT_MAX_ENTRIES: usize = 5000;

/// Boxed source of environment variables.
type EnvSource = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

fn cache_err<E: Display>(e: E) -> CoreError {
    CoreError::Cache(e.to_string())
}

/// [`ResultCache`] backed by a [`redb`] database.
///
/// The key ([`ResultCache::key`]) is the BLAKE3 hash of, in order:
/// the orq version and `up_to`; the `full_rebuild` flag; the relative path and
/// contents of every file of the crates in `changed ∪ dependents` (or of the
/// whole workspace on `full_rebuild`), taken from `git ls-files` (tracked plus
/// untracked-not-ignored) or, outside git, from a directory walk skipping
/// `target/` and hidden directories; the root `Cargo.lock`, `Cargo.toml`,
/// `rust-toolchain[.toml]` and `.cargo/config[.toml]` (presence included); the
/// output of `rustc -Vv` (memoized per instance, honours `RUSTC`); and the
/// environment variables `RUSTFLAGS`, `RUSTDOCFLAGS`, `RUSTC`, `RUSTC_WRAPPER`,
/// `RUSTC_WORKSPACE_WRAPPER`, `RUSTUP_TOOLCHAIN`, `CARGO_ENCODED_RUSTFLAGS`,
/// `CARGO_INCREMENTAL` and any `CARGO_BUILD_*`, `CARGO_PROFILE_*`,
/// `CARGO_TARGET_*`, `CARGO_CFG_*` (except the output-location ones
/// `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` and
/// `CARGO_BUILD_JOBS`), sorted by name.
///
/// Stored entries are bounded (default 5000) with least-recently-used
/// eviction.
pub struct RedbCache {
    db: Database,
    max_entries: usize,
    rustc_version: OnceLock<String>,
    env: EnvSource,
}

impl RedbCache {
    /// Opens `<root>/target/orq/cache.redb` (honouring `CARGO_TARGET_DIR`),
    /// falling back to `$XDG_CACHE_HOME/orq/<hash-of-root>/cache.redb` (or
    /// `~/.cache/...`) when that location is not writable.
    pub fn open(root: &Path) -> Result<Self, CoreError> {
        let target = match std::env::var_os("CARGO_TARGET_DIR") {
            Some(t) if !t.is_empty() => root.join(PathBuf::from(t)),
            _ => root.join("target"),
        };
        let primary = target.join("orq").join("cache.redb");
        match Self::open_at(&primary) {
            Ok(c) => Ok(c),
            Err(e) => {
                let fallback = fallback_path(root);
                tracing::warn!(
                    "cannot open cache at {} ({e}); falling back to {}",
                    primary.display(),
                    fallback.display()
                );
                Self::open_at(&fallback)
            }
        }
    }

    /// Opens (creating it if needed) the database at `db_path`.
    pub fn open_at(db_path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io(parent, e))?;
        }
        let db = Database::create(db_path).map_err(cache_err)?;
        Ok(Self {
            db,
            max_entries: DEFAULT_MAX_ENTRIES,
            rustc_version: OnceLock::new(),
            env: Box::new(|| std::env::vars().collect()),
        })
    }

    /// Sets the maximum number of entries (default 5000, minimum 1).
    pub fn with_max_entries(mut self, n: usize) -> Self {
        self.max_entries = n.max(1);
        self
    }

    /// Replaces the environment source used to compute keys (default: the
    /// process environment). Mainly useful in tests.
    pub fn with_env_source(
        mut self,
        source: impl Fn() -> Vec<(String, String)> + Send + Sync + 'static,
    ) -> Self {
        self.env = Box::new(source);
        self
    }

    /// Number of stored entries.
    pub fn len(&self) -> Result<usize, CoreError> {
        let txn = self.db.begin_read().map_err(cache_err)?;
        match txn.open_table(DATA) {
            Ok(t) => Ok(usize::try_from(t.len().map_err(cache_err)?).unwrap_or(usize::MAX)),
            Err(redb::TableError::TableDoesNotExist(_)) => Ok(0),
            Err(e) => Err(cache_err(e)),
        }
    }

    /// `true` if no entry is stored.
    pub fn is_empty(&self) -> Result<bool, CoreError> {
        Ok(self.len()? == 0)
    }

    fn rustc_version(&self, env: &[(String, String)]) -> Result<&str, CoreError> {
        if let Some(v) = self.rustc_version.get() {
            return Ok(v);
        }
        let program = env
            .iter()
            .find(|(k, _)| k == "RUSTC")
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "rustc".to_owned());
        let out = Command::new(&program)
            .arg("-Vv")
            .output()
            .map_err(|e| CoreError::Process {
                command: format!("{program} -Vv"),
                message: e.to_string(),
            })?;
        if !out.status.success() {
            return Err(CoreError::Process {
                command: format!("{program} -Vv"),
                message: format!("exited with {}", out.status),
            });
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        Ok(self.rustc_version.get_or_init(|| text))
    }
}

fn fallback_path(root: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        .unwrap_or_else(std::env::temp_dir);
    let hash = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
    base.join("orq")
        .join(&hash.as_str()[..16])
        .join("cache.redb")
}

/// Hashes a length-prefixed field so concatenations are unambiguous.
fn field(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

impl ResultCache for RedbCache {
    fn key(&self, root: &Path, impact: &ImpactSet, up_to: Step) -> Result<CacheKey, CoreError> {
        let env = relevant_env((self.env)());
        let rustc = self.rustc_version(&env)?;

        let mut h = blake3::Hasher::new();
        field(&mut h, b"orq-cache-key-v1");
        field(&mut h, ORQ_VERSION.as_bytes());
        field(&mut h, up_to.as_str().as_bytes());
        field(&mut h, &[u8::from(impact.full_rebuild)]);
        field(&mut h, rustc.as_bytes());
        for (k, v) in &env {
            field(&mut h, k.as_bytes());
            field(&mut h, v.as_bytes());
        }

        for name in GLOBAL_FILES {
            field(&mut h, name.as_bytes());
            match std::fs::read(root.join(name)) {
                Ok(bytes) => {
                    h.update(&[1]);
                    field(&mut h, &bytes);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    h.update(&[0]);
                }
                Err(e) => return Err(CoreError::io(root.join(name), e)),
            }
        }

        for rel in collect_files(root, impact)? {
            let abs = root.join(&rel);
            let Some(bytes) = file_fingerprint(&abs)? else {
                continue;
            };
            field(&mut h, rel.to_string_lossy().as_bytes());
            field(&mut h, &bytes);
        }
        Ok(CacheKey(*h.finalize().as_bytes()))
    }

    fn get(&self, key: &CacheKey) -> Result<Option<VerifyResult>, CoreError> {
        let txn = self.db.begin_write().map_err(cache_err)?;
        let result = {
            let data = txn.open_table(DATA).map_err(cache_err)?;
            let raw = data.get(key.as_bytes().as_slice()).map_err(cache_err)?;
            let parsed = raw.map(|v| serde_json::from_slice::<VerifyResult>(v.value()));
            match parsed {
                None => None,
                Some(Ok(r)) => {
                    touch(&txn, key)?;
                    Some(r)
                }
                Some(Err(e)) => {
                    tracing::warn!("discarding unreadable cache entry {key}: {e}");
                    None
                }
            }
        };
        txn.commit().map_err(cache_err)?;
        Ok(result)
    }

    fn put(&self, key: &CacheKey, result: &VerifyResult) -> Result<(), CoreError> {
        let bytes = serde_json::to_vec(result).map_err(cache_err)?;
        let txn = self.db.begin_write().map_err(cache_err)?;
        {
            let mut data = txn.open_table(DATA).map_err(cache_err)?;
            data.insert(key.as_bytes().as_slice(), bytes.as_slice())
                .map_err(cache_err)?;
            touch(&txn, key)?;

            let len = usize::try_from(data.len().map_err(cache_err)?).unwrap_or(usize::MAX);
            if len > self.max_entries {
                let mut access = txn.open_table(ACCESS).map_err(cache_err)?;
                let mut entries: Vec<(u64, Vec<u8>)> = Vec::with_capacity(len);
                for item in access.iter().map_err(cache_err)? {
                    let (k, v) = item.map_err(cache_err)?;
                    entries.push((v.value(), k.value().to_vec()));
                }
                entries.sort();
                let excess = len - self.max_entries;
                for (_, k) in entries.into_iter().take(excess) {
                    data.remove(k.as_slice()).map_err(cache_err)?;
                    access.remove(k.as_slice()).map_err(cache_err)?;
                }
            }
        }
        txn.commit().map_err(cache_err)
    }
}

/// What a listed path contributes to the key: file contents, `symlink:` +
/// target for symlinks (git tracks the link, not what it points to), or
/// `None` for paths that are gone or are directories (e.g. submodules).
fn file_fingerprint(abs: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    let meta = match std::fs::symlink_metadata(abs) {
        Ok(m) => m,
        // Tracked but deleted in the work tree: contributes nothing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CoreError::io(abs, e)),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(abs).map_err(|e| CoreError::io(abs, e))?;
        let mut bytes = b"symlink:".to_vec();
        bytes.extend_from_slice(target.to_string_lossy().as_bytes());
        return Ok(Some(bytes));
    }
    if meta.is_dir() {
        return Ok(None);
    }
    std::fs::read(abs)
        .map(Some)
        .map_err(|e| CoreError::io(abs, e))
}

/// Records `key` as the most recently used entry.
fn touch(txn: &redb::WriteTransaction, key: &CacheKey) -> Result<(), CoreError> {
    let mut meta = txn.open_table(META).map_err(cache_err)?;
    let next = meta
        .get(TICK_ROW)
        .map_err(cache_err)?
        .map_or(0, |v| v.value())
        + 1;
    meta.insert(TICK_ROW, next).map_err(cache_err)?;
    let mut access = txn.open_table(ACCESS).map_err(cache_err)?;
    access
        .insert(key.as_bytes().as_slice(), next)
        .map_err(cache_err)?;
    Ok(())
}
