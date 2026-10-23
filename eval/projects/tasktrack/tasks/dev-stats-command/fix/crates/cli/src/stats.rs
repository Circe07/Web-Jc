//! `stats`: counts by status, priority and tag.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use model::{Priority, Status, Task};

pub fn summary(tasks: &[Task], today: NaiveDate) -> String {
    if tasks.is_empty() {
        return "no tasks".to_string();
    }
    let count = |s: Status| tasks.iter().filter(|t| t.status == s).count();
    let overdue = tasks.iter().filter(|t| t.is_overdue(today)).count();
    let priorities: Vec<String> = Priority::ALL
        .iter()
        .rev()
        .map(|p| format!("{p} {}", tasks.iter().filter(|t| t.priority == *p).count()))
        .collect();
    let mut lines = vec![
        format!(
            "{} tasks: {} open, {} in progress, {} done",
            tasks.len(),
            count(Status::Open),
            count(Status::InProgress),
            count(Status::Done)
        ),
        format!("overdue: {overdue}"),
        format!("priority: {}", priorities.join(", ")),
    ];
    let mut tags: BTreeMap<&str, usize> = BTreeMap::new();
    for tag in tasks.iter().flat_map(|t| &t.tags) {
        *tags.entry(tag.as_str()).or_default() += 1;
    }
    if !tags.is_empty() {
        let mut sorted: Vec<(&str, usize)> = tags.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = sorted.iter().map(|(t, n)| format!("{t} {n}")).collect();
        lines.push(format!("tags: {}", parts.join(", ")));
    }
    lines.join("\n")
}
