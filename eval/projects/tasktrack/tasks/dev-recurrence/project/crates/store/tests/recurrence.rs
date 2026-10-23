//! `Store::complete`: finishing a recurring task schedules the next one.

use model::{parse_date, parse_quick_add, Recurrence, Status, TaskId};
use store::Store;

fn d(s: &str) -> chrono::NaiveDate {
    parse_date(s).unwrap()
}

#[test]
fn completing_a_recurring_task_adds_the_next_occurrence() {
    let mut s = Store::new();
    s.add(
        parse_quick_add("Pay rent +home !high every:month due:2026-10-31").unwrap(),
        d("2026-10-01"),
    );
    let next = s.complete(TaskId(1), d("2026-10-30")).unwrap();
    assert_eq!(next, Some(TaskId(2)));
    assert_eq!(s.get(TaskId(1)).unwrap().status, Status::Done);
    let t = s.get(TaskId(2)).unwrap();
    assert_eq!(t.title, "Pay rent");
    assert_eq!(t.status, Status::Open);
    assert_eq!(t.due, Some(d("2026-11-30")));
    assert_eq!(t.recur, Some(Recurrence::Monthly));
    assert!(t.has_tag("home"));
    assert_eq!(t.created, d("2026-10-30"));
}

#[test]
fn completing_a_plain_task_adds_nothing() {
    let mut s = Store::new();
    s.add(parse_quick_add("one off").unwrap(), d("2026-10-01"));
    assert_eq!(s.complete(TaskId(1), d("2026-10-02")).unwrap(), None);
    assert_eq!(s.len(), 1);
    assert!(s.complete(TaskId(7), d("2026-10-02")).is_err());
}

#[test]
fn completing_twice_does_not_schedule_twice() {
    let mut s = Store::new();
    s.add(
        parse_quick_add("Standup every:day due:2026-10-23").unwrap(),
        d("2026-10-01"),
    );
    assert_eq!(
        s.complete(TaskId(1), d("2026-10-23")).unwrap(),
        Some(TaskId(2))
    );
    assert_eq!(s.complete(TaskId(1), d("2026-10-23")).unwrap(), None);
    assert_eq!(s.len(), 2);
}

#[test]
fn old_files_without_recurrence_still_load_and_round_trip() {
    let old = r#"{"version": 1, "next_id": 2, "tasks": [
        {"id": 1, "title": "legacy", "created": "2026-01-01", "due": "2026-02-01"}
    ]}"#;
    let s = Store::from_json(old).unwrap();
    assert_eq!(s.get(TaskId(1)).unwrap().recur, None);
    assert_eq!(Store::from_json(&s.to_json().unwrap()).unwrap(), s);
}
