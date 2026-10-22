use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use orq_core::{
    BuildOptions, CacheKey, CrateId, ImpactSet, Outcome, ResultCache, Step, VerifyResult,
};
use tempfile::TempDir;

use crate::RedbCache;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            // Not `fs::copy`: on macOS it preserves the source mtime, and the
            // index tests rely on fresh copies being "just written".
            std::fs::write(dest, std::fs::read(entry.path()).unwrap()).unwrap();
        }
    }
}

struct Ws {
    _dir: TempDir,
    root: PathBuf,
}

fn workspace(git: bool) -> Ws {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("multi");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/multi");
    copy_dir(&fixture, &root);
    if git {
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
    }
    Ws { _dir: dir, root }
}

fn krate(ws: &Ws, name: &str) -> CrateId {
    CrateId {
        name: name.into(),
        manifest_path: ws.root.join("crates").join(name).join("Cargo.toml"),
    }
}

fn impact(ws: &Ws, changed: &[&str], dependents: &[&str]) -> ImpactSet {
    ImpactSet {
        changed: changed.iter().map(|n| krate(ws, n)).collect(),
        dependents: dependents.iter().map(|n| krate(ws, n)).collect(),
        full_rebuild: false,
    }
}

fn cache(dir: &TempDir) -> RedbCache {
    RedbCache::open_at(&dir.path().join("cache.redb"))
        .unwrap()
        .with_env_source(Vec::new)
}

fn append(path: &Path, text: &str) {
    let mut s = std::fs::read_to_string(path).unwrap();
    s.push_str(text);
    std::fs::write(path, s).unwrap();
}

fn sample(total_ms: u64) -> VerifyResult {
    VerifyResult {
        outcome: Outcome::Pass,
        steps: vec![],
        diagnostics: vec![],
        test_failures: vec![],
        impact: ImpactSet::default(),
        cache_hit: false,
        total_ms,
    }
}

fn key_of(c: &RedbCache, ws: &Ws, i: &ImpactSet) -> CacheKey {
    c.key(&ws.root, i, Step::Test, &BuildOptions::default())
        .unwrap()
}

fn key_scenarios(git: bool) {
    let ws = workspace(git);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["mid"], &["app"]);

    let k0 = key_of(&c, &ws, &i);
    assert_eq!(k0, key_of(&c, &ws, &i), "same state, same key");

    // A file of a non-affected crate does not matter.
    append(&ws.root.join("crates/leaf/src/lib.rs"), "\n// noise\n");
    assert_eq!(k0, key_of(&c, &ws, &i));

    // A new file in a non-affected crate neither.
    std::fs::write(ws.root.join("crates/leaf/src/extra.rs"), "// x").unwrap();
    assert_eq!(k0, key_of(&c, &ws, &i));

    // One byte in an affected crate changes the key.
    append(&ws.root.join("crates/mid/src/lib.rs"), " ");
    let k1 = key_of(&c, &ws, &i);
    assert_ne!(k0, k1);

    // So does a dependent...
    append(&ws.root.join("crates/app/src/lib.rs"), " ");
    let k2 = key_of(&c, &ws, &i);
    assert_ne!(k1, k2);

    // ...and a brand new (untracked) file in an affected crate.
    std::fs::write(ws.root.join("crates/mid/src/new.rs"), "// new").unwrap();
    let k3 = key_of(&c, &ws, &i);
    assert_ne!(k2, k3);

    // Renames change the key even with identical contents.
    std::fs::rename(
        ws.root.join("crates/mid/src/new.rs"),
        ws.root.join("crates/mid/src/renamed.rs"),
    )
    .unwrap();
    assert_ne!(k3, key_of(&c, &ws, &i));
}

#[test]
fn key_reacts_only_to_affected_crates_with_git() {
    key_scenarios(true);
}

#[test]
fn key_reacts_only_to_affected_crates_without_git() {
    key_scenarios(false);
}

#[test]
fn key_depends_on_up_to_and_full_rebuild() {
    let ws = workspace(true);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["mid"], &["app"]);
    let a = c
        .key(&ws.root, &i, Step::Check, &BuildOptions::default())
        .unwrap();
    let b = c
        .key(&ws.root, &i, Step::Test, &BuildOptions::default())
        .unwrap();
    assert_ne!(a, b);

    let full = ImpactSet {
        full_rebuild: true,
        ..i.clone()
    };
    let kf = key_of(&c, &ws, &full);
    assert_ne!(b, kf);
    // On a full rebuild every crate counts.
    append(&ws.root.join("crates/leaf/src/lib.rs"), " ");
    assert_ne!(kf, key_of(&c, &ws, &full));
}

#[test]
fn key_depends_on_global_files() {
    let ws = workspace(true);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["mid"], &[]);
    let k0 = key_of(&c, &ws, &i);

    append(&ws.root.join("Cargo.lock"), "\n");
    let k1 = key_of(&c, &ws, &i);
    assert_ne!(k0, k1);

    std::fs::write(
        ws.root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"stable\"\n",
    )
    .unwrap();
    let k2 = key_of(&c, &ws, &i);
    assert_ne!(k1, k2);

    std::fs::create_dir_all(ws.root.join(".cargo")).unwrap();
    std::fs::write(ws.root.join(".cargo/config.toml"), "[build]\njobs = 2\n").unwrap();
    assert_ne!(k2, key_of(&c, &ws, &i));
}

fn env_cache(db: &TempDir, vars: &[(&str, &str)]) -> RedbCache {
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    RedbCache::open_at(&db.path().join("cache.redb"))
        .unwrap()
        .with_env_source(move || vars.clone())
}

#[test]
fn key_depends_on_rustflags_and_relevant_env() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &["app"]);
    let db = tempfile::tempdir().unwrap();
    let key = |vars: &[(&str, &str)]| {
        let c = env_cache(&db, vars);
        c.key(&ws.root, &i, Step::Test, &BuildOptions::default())
            .unwrap()
    };

    let base = key(&[]);
    assert_ne!(base, key(&[("RUSTFLAGS", "-Dwarnings")]));
    assert_ne!(
        key(&[("RUSTFLAGS", "-Dwarnings")]),
        key(&[("RUSTFLAGS", "-Cdebuginfo=0")])
    );
    assert_ne!(base, key(&[("RUSTDOCFLAGS", "-Dwarnings")]));
    assert_ne!(base, key(&[("CARGO_PROFILE_DEV_OPT_LEVEL", "3")]));
    assert_ne!(base, key(&[("CARGO_BUILD_RUSTFLAGS", "-Zfoo")]));

    // Irrelevant variables and output-location ones are ignored.
    assert_eq!(base, key(&[("HOME", "/somewhere"), ("PATH", "/bin")]));
    assert_eq!(base, key(&[("CARGO_TARGET_DIR", "/tmp/elsewhere")]));
    assert_eq!(base, key(&[("CARGO_BUILD_JOBS", "3")]));

    // Order of the source does not matter.
    assert_eq!(
        key(&[("RUSTFLAGS", "a"), ("RUSTDOCFLAGS", "b")]),
        key(&[("RUSTDOCFLAGS", "b"), ("RUSTFLAGS", "a")])
    );
}

#[test]
fn key_depends_on_rustc_binary() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &[]);
    let db = tempfile::tempdir().unwrap();
    let c = env_cache(&db, &[("RUSTC", "definitely-not-a-rustc-binary")]);
    assert!(c
        .key(&ws.root, &i, Step::Test, &BuildOptions::default())
        .is_err());
}

#[test]
fn put_get_roundtrip_and_persistence() {
    let db = tempfile::tempdir().unwrap();
    let path = db.path().join("cache.redb");
    let key = CacheKey([1; 32]);
    {
        let c = RedbCache::open_at(&path).unwrap();
        assert!(c.get(&key).unwrap().is_none());
        assert!(c.is_empty().unwrap());
        c.put(&key, &sample(42)).unwrap();
        assert_eq!(c.get(&key).unwrap(), Some(sample(42)));
        c.put(&key, &sample(43)).unwrap();
        assert_eq!(c.len().unwrap(), 1);
    }
    let c = RedbCache::open_at(&path).unwrap();
    assert_eq!(c.get(&key).unwrap(), Some(sample(43)));
}

#[test]
fn lru_evicts_least_recently_used() {
    let db = tempfile::tempdir().unwrap();
    let c = RedbCache::open_at(&db.path().join("cache.redb"))
        .unwrap()
        .with_max_entries(3);
    let k = |n: u8| CacheKey([n; 32]);
    for n in 1..=3 {
        c.put(&k(n), &sample(u64::from(n))).unwrap();
    }
    // Touch 1 so that 2 becomes the oldest.
    assert!(c.get(&k(1)).unwrap().is_some());
    c.put(&k(4), &sample(4)).unwrap();
    assert_eq!(c.len().unwrap(), 3);
    assert!(c.get(&k(2)).unwrap().is_none(), "2 was the LRU entry");
    assert!(c.get(&k(1)).unwrap().is_some());
    assert!(c.get(&k(3)).unwrap().is_some());
    assert!(c.get(&k(4)).unwrap().is_some());

    // Never grows beyond the bound.
    for n in 10..30 {
        c.put(&k(n), &sample(0)).unwrap();
    }
    assert_eq!(c.len().unwrap(), 3);
}

#[test]
fn open_uses_target_orq_dir() {
    let ws = workspace(false);
    let _c = RedbCache::open(&ws.root).unwrap();
    // CARGO_TARGET_DIR may be set in the test environment; accept either.
    if std::env::var_os("CARGO_TARGET_DIR").is_none() {
        assert!(ws.root.join("target/orq/cache.redb").is_file());
    }
}

#[test]
fn key_is_fast_on_multi_fixture() {
    let ws = workspace(true);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["base"], &["mid", "app"]);
    // Warm up: memoizes `rustc -Vv`.
    let _ = key_of(&c, &ws, &i);
    let start = Instant::now();
    let _ = key_of(&c, &ws, &i);
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() < 100, "key took {elapsed:?}");
}

#[test]
fn key_depends_on_build_options() {
    let ws = workspace(false);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["leaf"], &[]);
    let plain = c
        .key(&ws.root, &i, Step::Test, &BuildOptions::default())
        .unwrap();
    let featured = BuildOptions {
        features: vec!["x".into()],
        ..BuildOptions::default()
    };
    let target = BuildOptions {
        target: Some("wasm32-unknown-unknown".into()),
        ..BuildOptions::default()
    };
    let a = c.key(&ws.root, &i, Step::Test, &featured).unwrap();
    let b = c.key(&ws.root, &i, Step::Test, &target).unwrap();
    assert_ne!(plain, a);
    assert_ne!(plain, b);
    assert_ne!(a, b);
}

/// A tracked symlink to a directory (ripgrep's `HomebrewFormula`) must not
/// break the key; its target is what counts.
#[cfg(unix)]
#[test]
fn tracked_symlink_to_directory_is_hashed_by_target() {
    let ws = workspace(true);
    let leaf = ws.root.join("crates/leaf");
    std::fs::create_dir_all(leaf.join("real")).unwrap();
    std::os::unix::fs::symlink("real", leaf.join("link")).unwrap();
    let status = Command::new("git")
        .args(["add", "-A"])
        .current_dir(&ws.root)
        .status()
        .unwrap();
    assert!(status.success());

    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let i = impact(&ws, &["leaf"], &[]);
    let first = key_of(&c, &ws, &i);
    assert_eq!(first, key_of(&c, &ws, &i));

    std::fs::remove_file(leaf.join("link")).unwrap();
    std::os::unix::fs::symlink("src", leaf.join("link")).unwrap();
    assert_ne!(first, key_of(&c, &ws, &i));
}

// ---- rustc memo (toolchain fingerprint) ----

fn set_mtime(path: &Path, secs_ago: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// Makes every file under `dir` (except `.git`) look old.
fn age_all(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".git" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            age_all(&entry.path());
        } else if entry.file_type().unwrap().is_file() {
            set_mtime(&entry.path(), 3600);
        }
    }
}

type Counter = std::sync::Arc<std::sync::atomic::AtomicUsize>;

/// A cache on `db` whose `RUSTC` is `fake` and whose runner only counts.
fn counting_cache(db: &TempDir, fake: &Path, calls: &Counter) -> RedbCache {
    let vars = vec![("RUSTC".to_owned(), fake.to_string_lossy().into_owned())];
    let calls = calls.clone();
    RedbCache::open_at(&db.path().join("cache.redb"))
        .unwrap()
        .with_env_source(move || vars.clone())
        .with_rustc_runner(move |_| {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(format!("fake rustc run {n}\n"))
        })
}

fn count(calls: &Counter) -> usize {
    calls.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn rustc_memo_survives_reopening_the_db() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &[]);
    let db = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let fake = tools.path().join("rustc");
    std::fs::write(&fake, "#!/bin/sh\necho fake\n").unwrap();
    let calls = Counter::default();

    let k1 = {
        let c = counting_cache(&db, &fake, &calls);
        let k = key_of(&c, &ws, &i);
        assert_eq!(count(&calls), 1);
        // Same instance: no new run either.
        assert_eq!(k, key_of(&c, &ws, &i));
        assert_eq!(count(&calls), 1);
        k
    };
    let c = counting_cache(&db, &fake, &calls);
    assert_eq!(k1, key_of(&c, &ws, &i));
    assert_eq!(count(&calls), 1, "second instance must not run rustc");
}

#[test]
fn rustc_memo_is_invalidated_by_fake_rustc_changes() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &[]);
    let db = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let fake = tools.path().join("rustc");
    std::fs::write(&fake, "#!/bin/sh\necho fake\n").unwrap();
    let calls = Counter::default();
    let key = || {
        let c = counting_cache(&db, &fake, &calls);
        key_of(&c, &ws, &i)
    };

    let k0 = key();
    assert_eq!(count(&calls), 1);
    assert_eq!(k0, {
        let c = counting_cache(&db, &fake, &calls);
        // The runner output differs per run, so an equal key proves reuse.
        key_of(&c, &ws, &i)
    });
    assert_eq!(count(&calls), 1);

    // mtime only.
    set_mtime(&fake, 7200);
    let k1 = key();
    assert_eq!(count(&calls), 2, "mtime change must rerun rustc");
    assert_ne!(k0, k1);

    // content (and size).
    std::fs::write(&fake, "#!/bin/sh\necho fake, but newer\n").unwrap();
    let k2 = key();
    assert_eq!(count(&calls), 3, "content change must rerun rustc");
    assert_ne!(k1, k2);

    // Stable again afterwards.
    assert_eq!(k2, key());
    assert_eq!(count(&calls), 3);
}

#[test]
fn rustc_memo_is_invalidated_by_rust_toolchain_file() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &[]);
    let db = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let fake = tools.path().join("rustc");
    std::fs::write(&fake, "x").unwrap();
    let calls = Counter::default();
    let key = || {
        let c = counting_cache(&db, &fake, &calls);
        key_of(&c, &ws, &i)
    };
    key();
    key();
    assert_eq!(count(&calls), 1);
    std::fs::write(
        ws.root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"stable\"\n",
    )
    .unwrap();
    key();
    assert_eq!(count(&calls), 2);
}

// ---- file hash index ----

#[test]
fn recently_written_files_are_never_indexed() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &["app"]);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    // Freshly copied: mtimes are "now", inside the 2 s window.
    let k0 = key_of(&c, &ws, &i);
    assert_eq!(c.indexed_files(), 0, "racy files must not be indexed");

    // Rewriting with the same size is still seen (always re-hashed).
    let lib = ws.root.join("crates/mid/src/lib.rs");
    let mut bytes = std::fs::read(&lib).unwrap();
    let last = bytes.len() - 1;
    bytes[last] = bytes[last].wrapping_add(1);
    std::fs::write(&lib, &bytes).unwrap();
    assert_ne!(k0, key_of(&c, &ws, &i));
    assert_eq!(c.indexed_files(), 0);
}

#[test]
fn settled_files_are_indexed_and_key_is_unchanged() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &["app"]);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    let k_cold = key_of(&c, &ws, &i);
    assert_eq!(c.indexed_files(), 0);

    age_all(&ws.root);
    let k_aged = key_of(&c, &ws, &i);
    assert!(c.indexed_files() > 0, "old files should be indexed");
    assert_eq!(k_cold, k_aged, "indexing must not change the key");
    assert_eq!(k_aged, key_of(&c, &ws, &i), "index hit gives the same key");

    // A fresh process (instance) with the warm db agrees too.
    drop(c);
    let c2 = cache(&db);
    assert_eq!(k_aged, key_of(&c2, &ws, &i));
}

#[test]
fn normal_edit_of_indexed_file_changes_the_key() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &["app"]);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    age_all(&ws.root);
    let k0 = key_of(&c, &ws, &i);
    assert!(c.indexed_files() > 0);

    // Edit: same size, new mtime (now).
    let lib = ws.root.join("crates/mid/src/lib.rs");
    let mut bytes = std::fs::read(&lib).unwrap();
    let last = bytes.len() - 1;
    bytes[last] = bytes[last].wrapping_add(1);
    std::fs::write(&lib, &bytes).unwrap();
    let k1 = key_of(&c, &ws, &i);
    assert_ne!(k0, k1);

    // Edit with a different size, mtime put back to an old value.
    append(&lib, "// more\n");
    set_mtime(&lib, 3600);
    assert_ne!(k1, key_of(&c, &ws, &i));
}

/// Documented limitation: an edit that keeps the size and restores the old
/// mtime is invisible where ctime is not available. On unix the ctime moves
/// on such a reset, so the edit *is* detected; this test pins that.
#[cfg(unix)]
#[test]
fn same_size_edit_with_restored_mtime_is_caught_by_ctime_on_unix() {
    let ws = workspace(true);
    let i = impact(&ws, &["mid"], &[]);
    let db = tempfile::tempdir().unwrap();
    let c = cache(&db);
    age_all(&ws.root);
    let k0 = key_of(&c, &ws, &i);
    let lib = ws.root.join("crates/mid/src/lib.rs");
    let mut bytes = std::fs::read(&lib).unwrap();
    let last = bytes.len() - 1;
    bytes[last] = bytes[last].wrapping_add(1);
    std::fs::write(&lib, &bytes).unwrap();
    set_mtime(&lib, 3600);
    assert_ne!(k0, key_of(&c, &ws, &i));
}
