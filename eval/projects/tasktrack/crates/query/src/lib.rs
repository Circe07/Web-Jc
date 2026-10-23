//! A small filter language for listing tasks, and sorting.
//!
//! A query is a list of terms; a task matches when every term matches.
//!
//! | term                | matches tasks …                      |
//! |---------------------|--------------------------------------|
//! | `status:open`       | with that status                     |
//! | `+work` / `-work`   | with / without the tag               |
//! | `!high`             | with at least that priority          |
//! | `due<2026-11-01`    | due before the date (also `due>`)    |
//! | `overdue`           | overdue relative to the query's date |
//! | `/regex/`           | whose title matches the regex        |
//! | any other word      | whose title contains it (any case)   |

mod sort;

use chrono::NaiveDate;
use model::{parse_date, Priority, Status, Task};
use regex::Regex;

pub use sort::{sort_tasks, SortKey};

/// Errors in a query string.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Model(#[from] model::ModelError),
    #[error("invalid regex `{0}`: {1}")]
    Regex(String, regex::Error),
    #[error("unknown field `{0}`")]
    UnknownField(String),
}

/// One condition of a query.
#[derive(Debug, Clone)]
pub enum Term {
    Status(Status),
    HasTag(String),
    LacksTag(String),
    MinPriority(Priority),
    DueBefore(NaiveDate),
    DueAfter(NaiveDate),
    Overdue,
    TitleRegex(Regex),
    TitleContains(String),
}

impl Term {
    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        match self {
            Term::Status(s) => task.status == *s,
            Term::HasTag(t) => task.has_tag(t),
            Term::LacksTag(t) => !task.has_tag(t),
            Term::MinPriority(p) => task.priority >= *p,
            Term::DueBefore(d) => task.due.is_some_and(|due| due < *d),
            Term::DueAfter(d) => task.due.is_some_and(|due| due > *d),
            Term::Overdue => task.is_overdue(today),
            Term::TitleRegex(re) => re.is_match(&task.title),
            Term::TitleContains(w) => task.title.to_lowercase().contains(w.as_str()),
        }
    }
}

/// A parsed query.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    /// Parses a query string. An empty string matches every task.
    pub fn parse(text: &str) -> Result<Query, QueryError> {
        let terms = tokenize(text)
            .into_iter()
            .map(|tok| parse_term(&tok))
            .collect::<Result<_, _>>()?;
        Ok(Query { terms })
    }

    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        self.terms.iter().all(|t| t.matches(task, today))
    }

    /// The tasks that match, in their original order.
    pub fn filter<'a>(&self, tasks: &'a [Task], today: NaiveDate) -> Vec<&'a Task> {
        tasks.iter().filter(|t| self.matches(t, today)).collect()
    }
}

/// Splits on whitespace, but keeps `/a regex with spaces/` as one token.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_regex = false;
    for c in text.chars() {
        match c {
            '/' if current.is_empty() && !in_regex => {
                in_regex = true;
                current.push(c);
            }
            '/' if in_regex => {
                in_regex = false;
                current.push(c);
                tokens.push(std::mem::take(&mut current));
            }
            c if c.is_whitespace() && !in_regex => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn parse_term(token: &str) -> Result<Term, QueryError> {
    if let Some(pattern) = token
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
    {
        return Regex::new(pattern)
            .map(Term::TitleRegex)
            .map_err(|e| QueryError::Regex(pattern.to_string(), e));
    }
    if let Some(tag) = token.strip_prefix('+') {
        return Ok(Term::HasTag(tag.to_lowercase()));
    }
    if let Some(tag) = token.strip_prefix('-') {
        return Ok(Term::LacksTag(tag.to_lowercase()));
    }
    if let Some(p) = token.strip_prefix('!') {
        return Ok(Term::MinPriority(p.parse()?));
    }
    if let Some(d) = token.strip_prefix("due<") {
        return Ok(Term::DueBefore(parse_date(d)?));
    }
    if let Some(d) = token.strip_prefix("due>") {
        return Ok(Term::DueAfter(parse_date(d)?));
    }
    if token.eq_ignore_ascii_case("overdue") {
        return Ok(Term::Overdue);
    }
    if let Some((field, value)) = token.split_once(':') {
        return match field {
            "status" => Ok(Term::Status(value.parse()?)),
            "tag" => Ok(Term::HasTag(value.to_lowercase())),
            _ => Err(QueryError::UnknownField(field.to_string())),
        };
    }
    Ok(Term::TitleContains(token.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{parse_quick_add, TaskId};

    fn day(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    pub(crate) fn tasks() -> Vec<Task> {
        let lines = [
            "Write quarterly report +work !high due:2026-10-30",
            "Water the plants +home",
            "Fix bike brakes +home !urgent due:2026-10-20",
            "Review pull request +work due:2026-11-05",
            "Book dentist !low",
        ];
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                Task::from_quick_add(
                    TaskId(i as u32 + 1),
                    parse_quick_add(l).unwrap(),
                    day("2026-10-01"),
                )
            })
            .collect()
    }

    fn ids(q: &str) -> Vec<u32> {
        let all = tasks();
        Query::parse(q)
            .unwrap()
            .filter(&all, day("2026-10-23"))
            .iter()
            .map(|t| t.id.0)
            .collect()
    }

    #[test]
    fn empty_query_matches_all() {
        assert_eq!(ids(""), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn tags_and_priority() {
        assert_eq!(ids("+work"), [1, 4]);
        assert_eq!(ids("-home !high"), [1]);
        assert_eq!(ids("tag:home"), [2, 3]);
    }

    #[test]
    fn dates() {
        assert_eq!(ids("due<2026-11-01"), [1, 3]);
        assert_eq!(ids("due>2026-10-25"), [1, 4]);
        assert_eq!(ids("overdue"), [3]);
    }

    #[test]
    fn text_and_regex() {
        assert_eq!(ids("report"), [1]);
        assert_eq!(ids("/^(Water|Book) /"), [2, 5]);
        assert_eq!(ids("/pull request/"), [4]);
    }

    #[test]
    fn errors() {
        assert!(matches!(
            Query::parse("colour:red"),
            Err(QueryError::UnknownField(_))
        ));
        assert!(matches!(Query::parse("/(/"), Err(QueryError::Regex(..))));
        assert!(matches!(Query::parse("!huge"), Err(QueryError::Model(_))));
    }
}
