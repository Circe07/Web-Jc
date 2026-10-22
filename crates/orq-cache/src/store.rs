//! The redb-backed [`ResultCache`] implementation.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use orq_core::{
    BuildOptions, CacheKey, CoreError, ImpactSet, ResultCache, Step, VerifyResult, ORQ_VERSION,
};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::index::{now_ns, Entry, Stamp};
use crate::inputs::{collect_files, relevant_env, GLOBAL_FILES};
use crate::toolchain;

/// Serialized `VerifyResult`s by key.
const DATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("results");
/// Last-access tick by key (monotonic counter, for LRU).
const ACCESS: TableDefinition<&[u8], u64> = TableDefinition::new("access");
/// Single-row table holding the last issued tick.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const TICK_ROW: &str = "tick";
/// Memoized `rustc -Vv` output by toolchain fingerprint (see `toolchain`).
const TOOLCHAIN: TableDefinition<&[u8], &[u8]> = TableDefinition::new("toolchain");
/// Per-file content hashes: key = root id (16 bytes) ++ relative path.
const FILES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("file_index");
/// Rows kept in the toolchain / file tables before they are reset.
const MAX_TOOLCHAINS: u64 = 32;
const MAX_INDEXED_FILES: u64 = 200_000;

/// Default maximum number of stored entries.
const DEFAULT_MAX_ENTRIES: usize = 5000;

/// Boxed source of environment variables.
type EnvSource = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

/// Boxed `rustc -Vv` runner: takes the program, returns its stdout.
type RustcRunner = Box<dyn Fn(&str) -> Result<String, CoreError> + Send + Sync>;

fn run_rustc(program: &str) -> Result<String, CoreError> {
    let out = Command::new(program)
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
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn cache_err<E: Display>(e: E) -> CoreError {
    CoreError::Cache(e.to_string())
}

/// [`ResultCache`] backed by a [`redb`] database.
///
/// The key ([`ResultCache::key`]) is the BLAKE3 hash of, in order:
/// the orq version and `up_to`; the `full_rebuild` flag; the relative path and
/// content hash of every file of the crates in `changed ∪ dependents` (or of the
/// whole workspace on `full_rebuild`), taken from `git ls-files` (tracked plus
/// untracked-not-ignored) or, outside git, from a directory walk skipping
/// `target/` and hidden directories; the root `Cargo.lock`, `Cargo.toml`,
/// `rust-toolchain[.toml]` and `.cargo/config[.toml]` (presence included); the
/// output of `rustc -Vv` (honours `RUSTC`; memoized in the database under a
/// toolchain fingerprint, see the `toolchain` module); and the
/// environment variables `RUSTFLAGS`, `RUSTDOCFLAGS`, `RUSTC`, `RUSTC_WRAPPER`,
/// `RUSTC_WORKSPACE_WRAPPER`, `RUSTUP_TOOLCHAIN`, `CARGO_ENCODED_RUSTFLAGS`,
/// `CARGO_INCREMENTAL` and any `CARGO_BUILD_*`, `CARGO_PROFILE_*`,
/// `CARGO_TARGET_*`, `CARGO_CFG_*` (except the output-location ones
/// `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` and
/// `CARGO_BUILD_JOBS`), sorted by name.
///
/// File content hashes are kept in the database and reused across processes
/// while size, mtime, ctime and inode are unchanged and the mtime is over 2 s
/// old (see the `index` module). The key format tag is `orq-cache-key-v2`.
///
/// Stored entries are bounded (default 5000) with least-recently-used
/// eviction.
pub struct RedbCache {
    db: Database,
    max_entries: usize,
    rustc_memo: Mutex<Option<([u8; 32], String)>>,
    rustc_runner: RustcRunner,
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
            rustc_memo: Mutex::new(None),
            rustc_runner: Box::new(run_rustc),
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

    /// Replaces the function that runs `rustc -Vv` (default: spawning the
    /// program). Mainly useful in tests, to count invocations.
    pub fn with_rustc_runner(
        mut self,
        runner: impl Fn(&str) -> Result<String, CoreError> + Send + Sync + 'static,
    ) -> Self {
        self.rustc_runner = Box::new(runner);
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

    /// Number of rows of the file hash index (tests only).
    #[cfg(test)]
    pub(crate) fn indexed_files(&self) -> usize {
        let Ok(txn) = self.db.begin_read() else {
            return 0;
        };
        match txn.open_table(FILES) {
            Ok(t) => usize::try_from(t.len().unwrap_or(0)).unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// `rustc -Vv`, from the in-process memo, the database or a fresh run,
    /// keyed by the toolchain fingerprint.
    fn rustc_version(&self, root: &Path, env: &[(String, String)]) -> Result<String, CoreError> {
        let program = env
            .iter()
            .find(|(k, _)| k == "RUSTC")
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "rustc".to_owned());
        let fp = toolchain::fingerprint(root, &program, env);
        if let Ok(memo) = self.rustc_memo.lock() {
            if let Some((f, text)) = memo.as_ref() {
                if *f == fp {
                    return Ok(text.clone());
                }
            }
        }
        let text = match self.stored_rustc(&fp) {
            Some(t) => t,
            None => {
                let t = (self.rustc_runner)(&program)?;
                if let Err(e) = self.store_rustc(&fp, &t) {
                    tracing::warn!("cannot persist rustc version: {e}");
                }
                t
            }
        };
        if let Ok(mut memo) = self.rustc_memo.lock() {
            *memo = Some((fp, text.clone()));
        }
        Ok(text)
    }

    fn stored_rustc(&self, fp: &[u8; 32]) -> Option<String> {
        let txn = self.db.begin_read().ok()?;
        let table = txn.open_table(TOOLCHAIN).ok()?;
        let v = table.get(fp.as_slice()).ok()??;
        String::from_utf8(v.value().to_vec()).ok()
    }

    fn store_rustc(&self, fp: &[u8; 32], text: &str) -> Result<(), CoreError> {
        let txn = self.db.begin_write().map_err(cache_err)?;
        {
            let mut t = txn.open_table(TOOLCHAIN).map_err(cache_err)?;
            if t.len().map_err(cache_err)? >= MAX_TOOLCHAINS {
                clear_table(&mut t)?;
            }
            t.insert(fp.as_slice(), text.as_bytes())
                .map_err(cache_err)?;
        }
        txn.commit().map_err(cache_err)
    }

    /// Content hashes of `files` (relative to `root`), in order, skipping
    /// paths that contribute nothing. Uses and refreshes the file index.
    fn hash_files(
        &self,
        root: &Path,
        files: &BTreeSet<PathBuf>,
    ) -> Result<Vec<(PathBuf, [u8; 32])>, CoreError> {
        let root_id = {
            let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let h = blake3::hash(canon.to_string_lossy().as_bytes());
            let mut id = [0u8; 16];
            id.copy_from_slice(&h.as_bytes()[..16]);
            id
        };
        let now = now_ns();
        let read = self.db.begin_read().map_err(cache_err)?;
        let table = match read.open_table(FILES) {
            Ok(t) => Some(t),
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(e) => return Err(cache_err(e)),
        };
        let mut out = Vec::with_capacity(files.len());
        let mut updates: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for rel in files {
            let abs = root.join(rel);
            let meta = match std::fs::symlink_metadata(&abs) {
                Ok(m) => m,
                // Tracked but deleted in the work tree: contributes nothing.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(CoreError::io(&abs, e)),
            };
            if meta.file_type().is_symlink() {
                // Git tracks the link, not what it points to.
                let target = std::fs::read_link(&abs).map_err(|e| CoreError::io(&abs, e))?;
                let mut bytes = b"symlink:".to_vec();
                bytes.extend_from_slice(target.to_string_lossy().as_bytes());
                out.push((rel.clone(), *blake3::hash(&bytes).as_bytes()));
                continue;
            }
            if meta.is_dir() {
                continue;
            }
            let stamp = Stamp::of(&meta);
            let mut key = root_id.to_vec();
            key.extend_from_slice(rel.to_string_lossy().as_bytes());
            let cached = match &table {
                Some(t) if stamp.settled(now) => t
                    .get(key.as_slice())
                    .map_err(cache_err)?
                    .and_then(|v| Entry::decode(v.value()))
                    .filter(|e| e.stamp == stamp),
                _ => None,
            };
            if let Some(e) = cached {
                out.push((rel.clone(), e.hash));
                continue;
            }
            let bytes = std::fs::read(&abs).map_err(|e| CoreError::io(&abs, e))?;
            let hash = *blake3::hash(&bytes).as_bytes();
            if stamp.settled(now) {
                updates.push((key, Entry { stamp, hash }.encode()));
            }
            out.push((rel.clone(), hash));
        }
        drop(table);
        drop(read);
        if !updates.is_empty() {
            if let Err(e) = self.store_hashes(&updates) {
                tracing::warn!("cannot update file index: {e}");
            }
        }
        Ok(out)
    }

    fn store_hashes(&self, updates: &[(Vec<u8>, Vec<u8>)]) -> Result<(), CoreError> {
        let txn = self.db.begin_write().map_err(cache_err)?;
        {
            let mut t = txn.open_table(FILES).map_err(cache_err)?;
            if t.len().map_err(cache_err)? >= MAX_INDEXED_FILES {
                clear_table(&mut t)?;
            }
            for (k, v) in updates {
                t.insert(k.as_slice(), v.as_slice()).map_err(cache_err)?;
            }
        }
        txn.commit().map_err(cache_err)
    }
}

/// Removes every row of `t`.
fn clear_table(t: &mut redb::Table<&[u8], &[u8]>) -> Result<(), CoreError> {
    let keys: Vec<Vec<u8>> = t
        .iter()
        .map_err(cache_err)?
        .filter_map(|r| r.ok().map(|(k, _)| k.value().to_vec()))
        .collect();
    for k in keys {
        t.remove(k.as_slice()).map_err(cache_err)?;
    }
    Ok(())
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
    fn key(
        &self,
        root: &Path,
        impact: &ImpactSet,
        up_to: Step,
        build: &BuildOptions,
    ) -> Result<CacheKey, CoreError> {
        let all_env = (self.env)();
        let rustc = self.rustc_version(root, &all_env)?;
        let env = relevant_env(all_env);

        let mut h = blake3::Hasher::new();
        field(&mut h, b"orq-cache-key-v2");
        field(&mut h, ORQ_VERSION.as_bytes());
        field(&mut h, up_to.as_str().as_bytes());
        field(&mut h, &[u8::from(impact.full_rebuild)]);
        for arg in build.cargo_args() {
            field(&mut h, arg.as_bytes());
        }
        field(&mut h, format!("{:?}", build.targets).as_bytes());
        // With `Targets::Auto` the runner builds benches/examples only when
        // one of them changed, so that choice is part of what was verified.
        for file in &impact.changed_files {
            let path = file.to_string_lossy();
            if path.contains("benches/") || path.contains("examples/") {
                field(&mut h, path.as_bytes());
            }
        }
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

        for (rel, hash) in self.hash_files(root, &collect_files(root, impact)?)? {
            field(&mut h, rel.to_string_lossy().as_bytes());
            h.update(&hash);
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
