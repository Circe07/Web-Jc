//! Plain-text rendering of task lists.

use chrono::NaiveDate;
use model::{Priority, Task};

/// A table with one row per task: id, status, priority, due, title and tags.
/// Returns `no tasks` for an empty list.
pub fn table(tasks: &[&Task], today: NaiveDate) -> String {
    if tasks.is_empty() {
        return "no tasks".to_string();
    }
    let rows: Vec<[String; 5]> = tasks.iter().map(|t| row(t, today)).collect();
    let mut widths = [0usize; 5];
    for r in &rows {
        for (w, cell) in widths.iter_mut().zip(r) {
            *w = (*w).max(cell.chars().count());
        }
    }
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = r
                .iter()
                .zip(widths)
                .enumerate()
                .map(|(i, (cell, w))| {
                    if i == r.len() - 1 {
                        cell.clone()
                    } else {
                        format!("{cell:<w$}")
                    }
                })
                .collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn row(task: &Task, today: NaiveDate) -> [String; 5] {
    let priority = match task.priority {
        Priority::Normal => String::new(),
        p => p.to_string(),
    };
    let tags: Vec<String> = task.tags.iter().map(ToString::to_string).collect();
    let mut title = task.title.clone();
    if !tags.is_empty() {
        title = format!("{title} {}", tags.join(" "));
    }
    [
        task.id.to_string(),
        task.status.to_string(),
        priority,
        due_label(task, today),
        title,
    ]
}

/// `2026-10-30 (in 7d)`, `2026-10-20 (3d late)`, `2026-10-23 (today)`, or empty.
pub fn due_label(task: &Task, today: NaiveDate) -> String {
    let (Some(due), Some(days)) = (task.due, task.days_left(today)) else {
        return String::new();
    };
    let note = match days {
        0 => "today".to_string(),
        d if d > 0 => format!("in {d}d"),
        d if !task.status.is_finished() => format!("{}d ago", -d),
        d => format!("{}d late", -d),
    };
    format!("{due} ({note})")
}
