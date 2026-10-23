//! `orq stats`: summary of `target/orq/stats.jsonl` (PLAN.md T3.3).
//! Records are written by `orq verify` and the MCP server through
//! `orq_core::stats`.
//!
//! Percentiles use the nearest-rank method: for `n` sorted values the
//! p-th percentile is the value at 1-based rank `ceil(p/100 * n)`. The median
//! is the p50 by that rule (the lower middle value for even counts), so it is
//! always an observed value.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use orq_core::root;
use orq_core::stats::{read_all, StatsRecord};
use serde_json::{json, Value};

/// Arguments of `orq stats`.
#[derive(Args)]
pub struct StatsArgs {
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
    /// Workspace root (default: the workspace containing the current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Only consider the last N records.
    #[arg(long, value_name = "N")]
    pub last: Option<usize>,
}

/// Estimated ratio between the tokens of raw `cargo build && cargo test`
/// output and the compact report. `StatsRecord` does not record the raw size,
/// so savings are an ESTIMATE from `docs/benchmarks.md`: about 40x on
/// `fixtures/multi` and 56x on ripgrep; the lower figure is used so the
/// estimate errs on the conservative side.
pub const RAW_TO_COMPACT_RATIO: u64 = 40;

/// Estimated savings of a set of records. Every figure is an estimate; see
/// [`RAW_TO_COMPACT_RATIO`] for the token basis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Savings {
    /// Estimated tokens of the raw cargo output the agent would have read.
    pub raw_tokens: u64,
    /// Tokens actually sent (sum over records).
    pub report_tokens: u64,
    /// `raw_tokens - report_tokens`.
    pub tokens_saved: u64,
    /// Per cache hit: median miss duration minus the hit's duration (never
    /// negative), summed over hits. Zero when there is no hit or no miss.
    pub time_saved_ms: u64,
    /// Cache hits counted in `time_saved_ms`.
    pub hits_counted: usize,
}

impl Savings {
    fn of(records: &[StatsRecord], miss_median_ms: u64, miss_count: usize) -> Self {
        let report_tokens: u64 = records
            .iter()
            .map(|r| u64::try_from(r.tokens).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add);
        let raw_tokens = report_tokens.saturating_mul(RAW_TO_COMPACT_RATIO);
        let (time_saved_ms, hits_counted) = if miss_count == 0 {
            (0, 0)
        } else {
            records
                .iter()
                .filter(|r| r.cache_hit)
                .fold((0u64, 0usize), |(sum, n), r| {
                    (
                        sum.saturating_add(miss_median_ms.saturating_sub(r.duration_ms)),
                        n + 1,
                    )
                })
        };
        Self {
            raw_tokens,
            report_tokens,
            tokens_saved: raw_tokens.saturating_sub(report_tokens),
            time_saved_ms,
            hits_counted,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "estimate": true,
            "basis": format!(
                "raw tokens = report tokens x {RAW_TO_COMPACT_RATIO} (docs/benchmarks.md); \
                 time = sum over cache hits of (median miss - hit duration)"
            ),
            "raw_tokens": self.raw_tokens,
            "report_tokens": self.report_tokens,
            "tokens_saved": self.tokens_saved,
            "time_saved_ms": self.time_saved_ms,
            "hits_counted": self.hits_counted,
        })
    }
}

/// Nearest-rank percentile (`p` in 0..=100) of an ascending-sorted slice.
/// Returns 0 for an empty slice.
pub fn percentile(sorted: &[u64], p: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = (usize::try_from(p).unwrap_or(100) * n).div_ceil(100).max(1);
    sorted[rank.min(n) - 1]
}

/// Duration distribution of a group of records, in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationStats {
    /// Number of records.
    pub count: usize,
    /// Median (nearest-rank p50).
    pub median_ms: u64,
    /// Nearest-rank p90.
    pub p90_ms: u64,
}

impl DurationStats {
    fn of(records: &[&StatsRecord]) -> Self {
        let mut d: Vec<u64> = records.iter().map(|r| r.duration_ms).collect();
        d.sort_unstable();
        Self {
            count: d.len(),
            median_ms: percentile(&d, 50),
            p90_ms: percentile(&d, 90),
        }
    }

    fn to_json(&self) -> Value {
        json!({"count": self.count, "median_ms": self.median_ms, "p90_ms": self.p90_ms})
    }
}

/// Aggregated statistics over a set of records.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// Number of records.
    pub count: usize,
    /// Records with outcome `pass`.
    pub pass: usize,
    /// Records with any other outcome.
    pub fail: usize,
    /// Duration of all records.
    pub duration: DurationStats,
    /// Fraction of records served from the cache (0.0..=1.0).
    pub cache_hit_rate: f64,
    /// Mean estimated tokens.
    pub avg_tokens: f64,
    /// Nearest-rank p90 of tokens.
    pub p90_tokens: u64,
    /// Duration of cache hits.
    pub hit: DurationStats,
    /// Duration of cache misses.
    pub miss: DurationStats,
    /// Duration of verifications served by the daemon.
    pub daemon: DurationStats,
    /// Duration of in-process verifications.
    pub in_process: DurationStats,
    /// Estimated savings versus raw cargo output and versus recomputing.
    pub savings: Savings,
}

/// Computes the [`Summary`] of `records` (an empty slice gives zeros).
pub fn summarize(records: &[StatsRecord]) -> Summary {
    let all: Vec<&StatsRecord> = records.iter().collect();
    let pick = |f: &dyn Fn(&StatsRecord) -> bool| -> Vec<&StatsRecord> {
        records.iter().filter(|r| f(r)).collect()
    };
    let count = records.len();
    let pass = records.iter().filter(|r| r.outcome == "pass").count();
    let hits = records.iter().filter(|r| r.cache_hit).count();
    let mut tokens: Vec<u64> = records
        .iter()
        .map(|r| u64::try_from(r.tokens).unwrap_or(u64::MAX))
        .collect();
    tokens.sort_unstable();
    let token_sum: f64 = tokens.iter().map(|&t| t as f64).sum();
    let (cache_hit_rate, avg_tokens) = if count == 0 {
        (0.0, 0.0)
    } else {
        (hits as f64 / count as f64, token_sum / count as f64)
    };
    let miss = DurationStats::of(&pick(&|r| !r.cache_hit));
    let savings = Savings::of(records, miss.median_ms, miss.count);
    Summary {
        count,
        pass,
        fail: count - pass,
        duration: DurationStats::of(&all),
        cache_hit_rate,
        avg_tokens,
        p90_tokens: percentile(&tokens, 90),
        hit: DurationStats::of(&pick(&|r| r.cache_hit)),
        miss,
        daemon: DurationStats::of(&pick(&|r| r.via_daemon)),
        in_process: DurationStats::of(&pick(&|r| !r.via_daemon)),
        savings,
    }
}

impl Summary {
    /// JSON form printed by `orq stats --json`.
    pub fn to_json(&self) -> Value {
        json!({
            "count": self.count,
            "pass": self.pass,
            "fail": self.fail,
            "duration_ms": {
                "median": self.duration.median_ms,
                "p90": self.duration.p90_ms,
                "method": "nearest-rank",
            },
            "cache_hit_rate": self.cache_hit_rate,
            "tokens": {"avg": self.avg_tokens, "p90": self.p90_tokens},
            "by_cache": {"hit": self.hit.to_json(), "miss": self.miss.to_json()},
            "by_mode": {
                "daemon": self.daemon.to_json(),
                "in_process": self.in_process.to_json(),
            },
            "savings": self.savings.to_json(),
        })
    }

    /// Compact text table printed by `orq stats`.
    pub fn render(&self) -> String {
        if self.count == 0 {
            return "no verifications recorded yet\n".to_string();
        }
        let mut out = String::new();
        out.push_str(&format!(
            "verifications  {} ({} pass, {} fail)\n",
            self.count, self.pass, self.fail
        ));
        out.push_str(&format!(
            "duration       median {} ms, p90 {} ms (nearest-rank)\n",
            self.duration.median_ms, self.duration.p90_ms
        ));
        out.push_str(&format!(
            "cache hit rate {:.1}%\n",
            self.cache_hit_rate * 100.0
        ));
        out.push_str(&format!(
            "tokens         avg {:.1}, p90 {}\n",
            self.avg_tokens, self.p90_tokens
        ));
        let mut groups = vec![("cache hit", &self.hit), ("cache miss", &self.miss)];
        if self.daemon.count > 0 && self.in_process.count > 0 {
            groups.push(("daemon", &self.daemon));
            groups.push(("in-process", &self.in_process));
        }
        let sv = &self.savings;
        out.push_str(&format!(
            "\nsavings (estimate)\n  tokens  ~{} raw cargo output vs {} sent: ~{} saved (raw = sent x {RAW_TO_COMPACT_RATIO}, see docs/benchmarks.md)\n",
            sv.raw_tokens, sv.report_tokens, sv.tokens_saved
        ));
        if sv.hits_counted > 0 {
            out.push_str(&format!(
                "  time    ~{} ms saved by {} cache hit(s) (median miss {} ms minus each hit)\n",
                sv.time_saved_ms, sv.hits_counted, self.miss.median_ms
            ));
        } else {
            out.push_str("  time    no cache hit to compare with a miss yet\n");
        }
        out.push_str("\nbreakdown          n  median_ms  p90_ms\n");
        for (name, d) in groups.into_iter().filter(|(_, d)| d.count > 0) {
            out.push_str(&format!(
                "{name:<13} {:>6} {:>10} {:>7}\n",
                d.count, d.median_ms, d.p90_ms
            ));
        }
        out
    }
}

/// Keeps only the last `n` records (all when `n` is `None`).
fn tail(mut records: Vec<StatsRecord>, n: Option<usize>) -> Vec<StatsRecord> {
    if let Some(n) = n {
        let skip = records.len().saturating_sub(n);
        records.drain(..skip);
    }
    records
}

/// Runs `orq stats` and returns the exit code.
pub fn run(args: &StatsArgs) -> Result<u8> {
    let root = root::resolve(args.root.as_deref())?;
    let records = tail(read_all(&root)?, args.last);
    let summary = summarize(&records);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary.to_json())?);
    } else {
        print!("{}", summary.render());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orq_core::Step;

    fn rec(duration_ms: u64, cache_hit: bool, via_daemon: bool, tokens: usize) -> StatsRecord {
        StatsRecord {
            timestamp_ms: 0,
            duration_ms,
            cache_hit,
            via_daemon,
            tokens,
            crates: vec![],
            outcome: "pass".into(),
            up_to: Step::Test,
            source: "cli".into(),
        }
    }

    #[test]
    fn percentile_odd_and_even() {
        assert_eq!(percentile(&[1, 2, 3, 4, 5], 50), 3);
        assert_eq!(percentile(&[1, 2, 3, 4], 50), 2);
        assert_eq!(percentile(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], 90), 9);
        assert_eq!(percentile(&[], 90), 0);
    }

    #[test]
    fn odd_count() {
        let r = [
            rec(30, false, false, 10),
            rec(10, false, false, 20),
            rec(20, false, false, 30),
        ];
        let s = summarize(&r);
        assert_eq!(s.count, 3);
        assert_eq!(s.duration.median_ms, 20);
        assert_eq!(s.duration.p90_ms, 30);
        assert!((s.avg_tokens - 20.0).abs() < 1e-9);
        assert_eq!(s.p90_tokens, 30);
    }

    #[test]
    fn even_count_uses_lower_middle() {
        let r = [
            rec(10, false, false, 1),
            rec(20, true, false, 1),
            rec(30, false, false, 1),
            rec(40, true, false, 1),
        ];
        let s = summarize(&r);
        assert_eq!(s.duration.median_ms, 20);
        assert!((s.cache_hit_rate - 0.5).abs() < 1e-9);
        assert_eq!(s.hit.median_ms, 20);
        assert_eq!(s.miss.median_ms, 10);
    }

    #[test]
    fn single_record() {
        let s = summarize(&[rec(7, false, true, 5)]);
        assert_eq!(s.duration.median_ms, 7);
        assert_eq!(s.duration.p90_ms, 7);
        assert_eq!(s.daemon.count, 1);
        assert_eq!(s.in_process.count, 0);
        assert!(s.cache_hit_rate.abs() < 1e-9);
    }

    #[test]
    fn all_hits() {
        let s = summarize(&[rec(1, true, false, 2), rec(3, true, false, 2)]);
        assert!((s.cache_hit_rate - 1.0).abs() < 1e-9);
        assert_eq!(s.miss.count, 0);
        assert_eq!(s.miss.median_ms, 0);
        assert!(s.render().contains("100.0%"));
    }

    #[test]
    fn savings_are_estimated_from_tokens_and_hit_vs_miss_time() {
        let r = [
            rec(1000, false, false, 10),
            rec(3000, false, false, 10),
            rec(100, true, false, 10),
            rec(5000, true, false, 10),
        ];
        let s = summarize(&r);
        // miss median (nearest-rank, lower middle) = 1000
        assert_eq!(s.miss.median_ms, 1000);
        assert_eq!(s.savings.report_tokens, 40);
        assert_eq!(s.savings.raw_tokens, 40 * RAW_TO_COMPACT_RATIO);
        assert_eq!(s.savings.tokens_saved, 40 * RAW_TO_COMPACT_RATIO - 40);
        // hit 1: 1000-100 = 900; hit 2 is slower than the median miss -> 0
        assert_eq!(s.savings.time_saved_ms, 900);
        assert_eq!(s.savings.hits_counted, 2);
        let text = s.render();
        assert!(text.contains("savings (estimate)"));
        assert!(text.contains("900 ms saved"));
        assert_eq!(s.to_json()["savings"]["estimate"], true);
    }

    #[test]
    fn no_time_saving_without_a_miss() {
        let s = summarize(&[rec(5, true, false, 3)]);
        assert_eq!(s.savings.time_saved_ms, 0);
        assert_eq!(s.savings.hits_counted, 0);
        assert!(s.render().contains("no cache hit to compare"));
    }

    #[test]
    fn empty_and_last() {
        let s = summarize(&[]);
        assert_eq!(s.render(), "no verifications recorded yet\n");
        let r = tail(
            vec![
                rec(1, false, false, 1),
                rec(2, false, false, 1),
                rec(3, false, false, 1),
            ],
            Some(2),
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].duration_ms, 2);
    }
}
