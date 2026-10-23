//! Aggregation and the `offline.json` / `offline.md` outputs.

use std::fmt::Write as _;

use serde::Serialize;

use crate::run::{CaseResult, ModeResult};

/// Aggregate of one mode over all cases.
#[derive(Debug, Clone, Serialize)]
pub struct ModeSummary {
    /// Mode name.
    pub mode: String,
    /// Cases measured.
    pub cases: usize,
    /// Mean estimated tokens.
    pub mean_tokens: f64,
    /// Mean fact recall.
    pub mean_recall: f64,
    /// Percentage of cases with full recall.
    pub full_recall_pct: f64,
    /// Percentage of applicable cases whose first error is correct.
    pub first_error_pct: Option<f64>,
    /// Percentage of cases where orq blamed the right step (orq modes only).
    pub fail_step_pct: Option<f64>,
}

/// A failed case: it is reported, not measured.
#[derive(Debug, Clone, Serialize)]
pub struct CaseError {
    /// Case id.
    pub id: String,
    /// What went wrong.
    pub error: String,
}

/// The whole run, as written to `offline.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Budgets used.
    pub budgets: Vec<usize>,
    /// One summary per mode.
    pub summary: Vec<ModeSummary>,
    /// Per-case measurements.
    pub cases: Vec<CaseResult>,
    /// Cases that could not be run.
    pub errors: Vec<CaseError>,
}

fn pct(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * count as f64 / total as f64
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        0.0
    } else {
        sum / n as f64
    }
}

fn mode_of<'a>(case: &'a CaseResult, mode: &str) -> Option<&'a ModeResult> {
    case.modes.iter().find(|m| m.mode == mode)
}

/// Mode names in order of appearance.
fn mode_names(cases: &[CaseResult]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for case in cases {
        for m in &case.modes {
            if !names.contains(&m.mode) {
                names.push(m.mode.clone());
            }
        }
    }
    names
}

impl Report {
    /// Builds the report (summaries included) from per-case results.
    pub fn new(budgets: Vec<usize>, cases: Vec<CaseResult>, errors: Vec<CaseError>) -> Self {
        let summary = mode_names(&cases)
            .into_iter()
            .map(|mode| {
                let rows: Vec<(&CaseResult, &ModeResult)> = cases
                    .iter()
                    .filter_map(|c| mode_of(c, &mode).map(|m| (c, m)))
                    .collect();
                let applicable: Vec<bool> = rows
                    .iter()
                    .filter_map(|(_, m)| m.first_error_correct)
                    .collect();
                let is_orq = mode.starts_with("orq@");
                ModeSummary {
                    cases: rows.len(),
                    mean_tokens: mean(rows.iter().map(|(_, m)| m.tokens as f64)),
                    mean_recall: mean(rows.iter().map(|(_, m)| m.fact_recall)),
                    full_recall_pct: pct(
                        rows.iter()
                            .filter(|(_, m)| m.facts_found == m.facts_total)
                            .count(),
                        rows.len(),
                    ),
                    first_error_pct: (!applicable.is_empty())
                        .then(|| pct(applicable.iter().filter(|b| **b).count(), applicable.len())),
                    fail_step_pct: is_orq.then(|| {
                        pct(
                            rows.iter().filter(|(c, _)| c.fail_step_correct).count(),
                            rows.len(),
                        )
                    }),
                    mode,
                }
            })
            .collect();
        Self {
            budgets,
            summary,
            cases,
            errors,
        }
    }

    /// Mean recall of orq at the largest budget; `None` without data.
    pub fn orq_mean_recall_at_largest_budget(&self) -> Option<f64> {
        let budget = self.budgets.iter().max()?;
        let mode = crate::run::orq_mode(*budget);
        self.summary
            .iter()
            .find(|s| s.mode == mode)
            .map(|s| s.mean_recall)
    }

    /// Renders `offline.md`.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "# orq offline evaluation\n");
        let _ = writeln!(
            md,
            "{} cases, budgets {:?}. Tokens are `estimate_tokens` (ceil(chars / 4)). \
             `raw-step` is the cargo command of the failing step, `raw-ladder` is \
             `cargo build && cargo test`, `orq@N` is orq's compact report at budget N.\n",
            self.cases.len(),
            self.budgets
        );

        let _ = writeln!(md, "## Summary\n");
        let _ = writeln!(
            md,
            "| mode | cases | mean tokens | vs raw-step | mean recall | full recall | first error ok | fail step ok |"
        );
        let _ = writeln!(md, "|---|---:|---:|---:|---:|---:|---:|---:|");
        let raw_tokens = self
            .summary
            .iter()
            .find(|s| s.mode == crate::run::RAW_STEP)
            .map(|s| s.mean_tokens);
        for s in &self.summary {
            let ratio = match raw_tokens {
                Some(raw) if raw > 0.0 => format!("{:.0}%", 100.0 * s.mean_tokens / raw),
                _ => "-".into(),
            };
            let _ = writeln!(
                md,
                "| {} | {} | {:.0} | {} | {:.3} | {:.0}% | {} | {} |",
                s.mode,
                s.cases,
                s.mean_tokens,
                ratio,
                s.mean_recall,
                s.full_recall_pct,
                opt_pct(s.first_error_pct),
                opt_pct(s.fail_step_pct),
            );
        }

        let _ = writeln!(md, "\n## Per case\n");
        let names = mode_names(&self.cases);
        let _ = write!(md, "| case | category | fails at | orq says |");
        for n in &names {
            let _ = write!(md, " {n} |");
        }
        let _ = writeln!(md);
        let _ = write!(md, "|---|---|---|---|");
        for _ in &names {
            let _ = write!(md, "---:|");
        }
        let _ = writeln!(md);
        for c in &self.cases {
            let says = c.orq_failed_step.as_deref().unwrap_or("pass");
            let flag = if c.fail_step_correct { "" } else { " (WRONG)" };
            let _ = write!(
                md,
                "| {} | {} | {} | {}{} |",
                c.id, c.category, c.expected_fail_step, says, flag
            );
            for n in &names {
                match mode_of(c, n) {
                    Some(m) => {
                        let _ = write!(
                            md,
                            " {} tok, {}/{} |",
                            m.tokens, m.facts_found, m.facts_total
                        );
                    }
                    None => {
                        let _ = write!(md, " - |");
                    }
                }
            }
            let _ = writeln!(md);
        }
        let _ = writeln!(md, "\nCells are `tokens, facts found/facts total`.");

        let _ = writeln!(md, "\n## Facts orq lost\n");
        let _ = writeln!(
            md,
            "Facts present in the raw output of the failing step but missing from orq's report. \
             Each line points at a possible compressor bug.\n"
        );
        let mut any = false;
        for c in &self.cases {
            for l in &c.lost_facts {
                any = true;
                let _ = writeln!(md, "- `{}` at `{}`: {}", c.id, l.mode, l.fact);
            }
        }
        if !any {
            let _ = writeln!(md, "None: orq kept every fact the raw output had.");
        }

        let wrong_first: Vec<_> = self
            .cases
            .iter()
            .flat_map(|c| {
                c.modes
                    .iter()
                    .filter(|m| m.mode.starts_with("orq@") && m.first_error_correct == Some(false))
                    .map(move |m| format!("- `{}` at `{}`", c.id, m.mode))
            })
            .collect();
        if !wrong_first.is_empty() {
            let _ = writeln!(md, "\n## First error not the expected one\n");
            for line in wrong_first {
                let _ = writeln!(md, "{line}");
            }
        }

        if !self.errors.is_empty() {
            let _ = writeln!(md, "\n## Cases that failed to run\n");
            for e in &self.errors {
                let _ = writeln!(md, "- `{}`: {}", e.id, e.error);
            }
        }
        md
    }
}

fn opt_pct(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |v| format!("{v:.0}%"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::LostFact;

    fn mode(name: &str, tokens: usize, found: usize, total: usize) -> ModeResult {
        ModeResult {
            mode: name.into(),
            tokens,
            fact_recall: found as f64 / total as f64,
            facts_found: found,
            facts_total: total,
            missing: Vec::new(),
            first_error_correct: Some(found == total),
            truncated: None,
        }
    }

    fn case(id: &str, orq_found: usize) -> CaseResult {
        CaseResult {
            id: id.into(),
            category: "c".into(),
            difficulty: "easy".into(),
            expected_fail_step: "check".into(),
            orq_outcome: "fail".into(),
            orq_failed_step: Some("check".into()),
            fail_step_correct: true,
            modes: vec![
                mode("raw-step", 400, 2, 2),
                mode("orq@500", 100, orq_found, 2),
            ],
            lost_facts: if orq_found < 2 {
                vec![LostFact {
                    mode: "orq@500".into(),
                    fact: "E0308 (missing label)".into(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn summary_and_markdown() {
        let report = Report::new(vec![500], vec![case("a", 2), case("b", 1)], Vec::new());
        let orq = report.summary.iter().find(|s| s.mode == "orq@500").unwrap();
        assert!((orq.mean_tokens - 100.0).abs() < 1e-9);
        assert!((orq.mean_recall - 0.75).abs() < 1e-9);
        assert!((orq.full_recall_pct - 50.0).abs() < 1e-9);
        assert_eq!(orq.fail_step_pct, Some(100.0));
        assert_eq!(report.orq_mean_recall_at_largest_budget(), Some(0.75));
        let md = report.to_markdown();
        assert!(md.contains("| orq@500 | 2 | 100 | 25% |"));
        assert!(md.contains("`b` at `orq@500`: E0308 (missing label)"));
    }

    #[test]
    fn empty_report_has_no_recall() {
        let report = Report::new(vec![500], Vec::new(), Vec::new());
        assert_eq!(report.orq_mean_recall_at_largest_budget(), None);
        assert!(report.to_markdown().contains("None: orq kept every fact"));
    }
}
