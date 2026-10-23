//! Quick-add syntax: `Buy milk +shopping !high due:2026-11-01`.
//!
//! Words starting with `+` are tags, `!` sets the priority and `due:` the due
//! date. Every other word is part of the title, in order.

use std::collections::BTreeSet;

use chrono::NaiveDate;

use crate::{parse_date, ModelError, Priority, Tag};

/// The parts of a quick-add line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickAdd {
    pub title: String,
    pub priority: Option<Priority>,
    pub tags: BTreeSet<Tag>,
    pub due: Option<NaiveDate>,
}

/// Parses a quick-add line.
pub fn parse_quick_add(line: &str) -> Result<QuickAdd, ModelError> {
    let mut out = QuickAdd::default();
    let mut words = Vec::new();
    for word in line.split_whitespace() {
        if let Some(tag) = word.strip_prefix('+') {
            out.tags.insert(Tag::new(tag)?);
        } else if let Some(priority) = word.strip_prefix('!') {
            out.priority = Some(priority.parse()?);
        } else if let Some(date) = word.strip_prefix("due:") {
            out.due = Some(parse_date(date)?);
        } else {
            words.push(word);
        }
    }
    if words.is_empty() {
        return Err(ModelError::EmptyTitle);
    }
    out.title = words.join(" ");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_parts() {
        let q = parse_quick_add("Buy milk +shopping !high due:2026-11-01 +home").unwrap();
        assert_eq!(q.title, "Buy milk");
        assert_eq!(q.priority, Some(Priority::High));
        let tags: Vec<&str> = q.tags.iter().map(Tag::as_str).collect();
        assert_eq!(tags, ["home", "shopping"]);
        assert_eq!(q.due, Some(parse_date("2026-11-01").unwrap()));
    }

    #[test]
    fn title_only() {
        let q = parse_quick_add("  call   mum ").unwrap();
        assert_eq!(q.title, "call mum");
        assert!(q.tags.is_empty() && q.priority.is_none() && q.due.is_none());
    }

    #[test]
    fn errors() {
        assert_eq!(parse_quick_add("+work !low"), Err(ModelError::EmptyTitle));
        assert!(matches!(
            parse_quick_add("x !loud"),
            Err(ModelError::UnknownPriority(_))
        ));
        assert!(matches!(
            parse_quick_add("x due:tomorrow"),
            Err(ModelError::InvalidDate(_))
        ));
    }
}
