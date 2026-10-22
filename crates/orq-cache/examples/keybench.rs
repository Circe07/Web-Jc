//! Times `RedbCache::key`: `keybench <root> <db> <full|crate,crate,...> <runs>`.
//! Prints the first-call time and the median of the remaining calls.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    clippy::print_stdout
)]

use std::path::PathBuf;
use std::time::Instant;

use orq_cache::RedbCache;
use orq_core::{BuildOptions, CrateId, ImpactSet, ResultCache, Step};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    let c = RedbCache::open_at(&PathBuf::from(&a[2])).unwrap();
    let runs: usize = a[4].parse().unwrap();
    let impact = if a[3] == "full" {
        ImpactSet {
            full_rebuild: true,
            ..ImpactSet::default()
        }
    } else {
        ImpactSet {
            changed: a[3]
                .split(',')
                .map(|n| CrateId {
                    name: n.into(),
                    manifest_path: root.join("crates").join(n).join("Cargo.toml"),
                })
                .collect(),
            ..ImpactSet::default()
        }
    };
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..runs {
        let t = Instant::now();
        last = Some(
            c.key(&root, &impact, Step::Test, &BuildOptions::default())
                .unwrap(),
        );
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let first = times[0];
    let mut rest = times[1..].to_vec();
    rest.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let med = rest.get(rest.len() / 2).copied().unwrap_or(0.0);
    println!(
        "first {first:.1} ms, warm median {med:.2} ms, key {}",
        last.unwrap()
    );
}
