//! Domain types for tasktrack: tasks, priorities, statuses and tags, plus the
//! quick-add syntax used on the command line.

mod parse;

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub use parse::{parse_quick_add, QuickAdd};

/// Identifier of a task inside a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u32);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// How urgent a task is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Priority; 4] = [
        Priority::Low,
        Priority::Normal,
        Priority::High,
        Priority::Urgent,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
            Priority::Urgent => "urgent",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Priority {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Priority::ALL
            .into_iter()
            .find(|p| p.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| ModelError::UnknownPriority(s.to_string()))
    }
}

/// Where a task is in its life cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Open,
    InProgress,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::InProgress => "in-progress",
            Status::Done => "done",
        }
    }

    pub fn is_finished(self) -> bool {
        self == Status::Done
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Status {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "open" => Ok(Status::Open),
            "in-progress" | "doing" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            _ => Err(ModelError::UnknownStatus(s.to_string())),
        }
    }
}

/// A lowercase label without spaces, such as `work` or `home`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

impl Tag {
    pub fn new(raw: &str) -> Result<Tag, ModelError> {
        let name = raw.trim().to_lowercase();
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid {
            Ok(Tag(name))
        } else {
            Err(ModelError::InvalidTag(raw.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Tag {
    type Error = ModelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Tag::new(&value)
    }
}

impl From<Tag> for String {
    fn from(tag: Tag) -> String {
        tag.0
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "+{}", self.0)
    }
}

/// Errors raised while building domain values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("task title is empty")]
    EmptyTitle,
    #[error("unknown priority `{0}`")]
    UnknownPriority(String),
    #[error("unknown status `{0}`")]
    UnknownStatus(String),
    #[error("invalid tag `{0}`")]
    InvalidTag(String),
    #[error("invalid date `{0}` (expected YYYY-MM-DD)")]
    InvalidDate(String),
}

/// One task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub tags: BTreeSet<Tag>,
    #[serde(default)]
    pub due: Option<NaiveDate>,
    pub created: NaiveDate,
}

impl Task {
    /// A new open task with normal priority, no tags and no due date.
    pub fn new(id: TaskId, title: &str, created: NaiveDate) -> Task {
        Task {
            id,
            title: title.trim().to_string(),
            priority: Priority::default(),
            status: Status::default(),
            tags: BTreeSet::new(),
            due: None,
            created,
        }
    }

    /// Builds a task from parsed quick-add input.
    pub fn from_quick_add(id: TaskId, input: QuickAdd, created: NaiveDate) -> Task {
        Task {
            id,
            title: input.title,
            priority: input.priority.unwrap_or_default(),
            status: Status::Open,
            tags: input.tags,
            due: input.due,
            created,
        }
    }

    pub fn has_tag(&self, name: &str) -> bool {
        self.tags.iter().any(|t| t.as_str() == name)
    }

    /// True when the task is not done and its due date is before `today`.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.status.is_finished() && self.due.is_some_and(|d| d < today)
    }

    /// Days left until the due date (negative when overdue).
    pub fn days_left(&self, today: NaiveDate) -> Option<i64> {
        self.due.map(|d| (d - today).num_days())
    }
}

impl fmt::Display for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}] {}", self.id, self.status, self.title)?;
        if self.priority != Priority::Normal {
            write!(f, " !{}", self.priority)?;
        }
        for tag in &self.tags {
            write!(f, " {tag}")?;
        }
        if let Some(due) = self.due {
            write!(f, " due:{due}")?;
        }
        Ok(())
    }
}

/// Parses a `YYYY-MM-DD` date.
pub fn parse_date(text: &str) -> Result<NaiveDate, ModelError> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| ModelError::InvalidDate(text.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    #[test]
    fn priorities_parse_and_order() {
        assert_eq!("HIGH".parse::<Priority>(), Ok(Priority::High));
        assert!("meh".parse::<Priority>().is_err());
        assert!(Priority::Urgent > Priority::Low);
    }

    #[test]
    fn statuses_parse() {
        assert_eq!("doing".parse::<Status>(), Ok(Status::InProgress));
        assert_eq!(Status::InProgress.to_string(), "in-progress");
    }

    #[test]
    fn tags_are_normalised() {
        assert_eq!(Tag::new(" Work ").unwrap().as_str(), "work");
        assert!(Tag::new("two words").is_err());
        assert!(Tag::new("").is_err());
    }

    #[test]
    fn overdue_and_days_left() {
        let mut t = Task::new(TaskId(1), "pay rent", day("2026-10-01"));
        t.due = Some(day("2026-10-20"));
        assert!(t.is_overdue(day("2026-10-21")));
        assert!(!t.is_overdue(day("2026-10-20")));
        assert_eq!(t.days_left(day("2026-10-18")), Some(2));
        t.status = Status::Done;
        assert!(!t.is_overdue(day("2026-10-21")));
    }

    #[test]
    fn display_shows_everything_but_normal_priority() {
        let mut t = Task::new(TaskId(7), "ship it", day("2026-10-01"));
        t.priority = Priority::High;
        t.tags.insert(Tag::new("work").unwrap());
        t.due = Some(day("2026-10-31"));
        assert_eq!(
            t.to_string(),
            "#7 [open] ship it !high +work due:2026-10-31"
        );
    }
}
