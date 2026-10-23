//! Sorting of task lists.

use std::cmp::Ordering;
use std::str::FromStr;

use model::Task;

use crate::QueryError;

/// What to sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// By id (creation order).
    Id,
    /// Most urgent first, then by id.
    Priority,
    /// Earliest due date first; tasks without a due date go last.
    Due,
    /// Alphabetically by title, ignoring case.
    Title,
}

impl FromStr for SortKey {
    type Err = QueryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "id" => Ok(SortKey::Id),
            "priority" => Ok(SortKey::Priority),
            "due" => Ok(SortKey::Due),
            "title" => Ok(SortKey::Title),
            _ => Err(QueryError::UnknownField(s.to_string())),
        }
    }
}

/// Sorts in place; ties keep id order.
pub fn sort_tasks(tasks: &mut [&Task], key: SortKey) {
    tasks.sort_by(|a, b| compare(a, b, key).then(a.id.cmp(&b.id)));
}

fn compare(a: &Task, b: &Task, key: SortKey) -> Ordering {
    match key {
        SortKey::Id => Ordering::Equal,
        SortKey::Priority => b.priority.cmp(&a.priority),
        SortKey::Due => match (a.due, b.due) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        },
        SortKey::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::tasks;

    fn sorted(key: SortKey) -> Vec<u32> {
        let all = tasks();
        let mut refs: Vec<&Task> = all.iter().collect();
        sort_tasks(&mut refs, key);
        refs.iter().map(|t| t.id.0).collect()
    }

    #[test]
    fn by_priority() {
        assert_eq!(sorted(SortKey::Priority), [3, 1, 2, 4, 5]);
    }

    #[test]
    fn by_due_puts_undated_last() {
        assert_eq!(sorted(SortKey::Due), [3, 1, 4, 2, 5]);
    }

    #[test]
    fn by_title() {
        assert_eq!(sorted(SortKey::Title), [5, 3, 4, 2, 1]);
    }

    #[test]
    fn parses_keys() {
        assert_eq!("due".parse::<SortKey>().unwrap(), SortKey::Due);
        assert!("size".parse::<SortKey>().is_err());
    }
}
