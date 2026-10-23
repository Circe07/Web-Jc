//! `or` between groups of terms: a task matches when any group matches.

use chrono::NaiveDate;
use model::{parse_date, parse_quick_add, Task, TaskId};
use query::Query;

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn tasks() -> Vec<Task> {
    [
        "Write quarterly report +work !high due:2026-10-30",
        "Water the plants +home",
        "Fix bike brakes +home !urgent due:2026-10-20",
        "Review pull request +work due:2026-11-05",
        "Book dentist !low",
    ]
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
fn either_group_matches() {
    assert_eq!(ids("+work or dentist"), [1, 4, 5]);
}

#[test]
fn terms_in_a_group_are_still_anded() {
    assert_eq!(ids("+home !urgent or +work due>2026-11-01"), [3, 4]);
}

#[test]
fn or_is_case_insensitive_and_can_repeat() {
    assert_eq!(ids("dentist OR plants Or overdue"), [2, 3, 5]);
}

#[test]
fn quoted_regex_may_contain_or() {
    assert_eq!(ids("/report or review/"), Vec::<u32>::new());
    assert_eq!(ids("/(report|request)/"), [1, 4]);
}

#[test]
fn empty_groups_are_errors() {
    assert!(Query::parse("or +work").is_err());
    assert!(Query::parse("+work or").is_err());
    assert!(Query::parse("+work or or +home").is_err());
}

#[test]
fn single_group_behaves_as_before() {
    assert_eq!(ids("+home"), [2, 3]);
    assert_eq!(ids(""), [1, 2, 3, 4, 5]);
}
