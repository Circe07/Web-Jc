//! Recurring tasks: `every:day|week|month` in quick-add.

use model::{parse_date, parse_quick_add, ModelError, Recurrence, Task, TaskId};

#[test]
fn parses_recurrence_names() {
    assert_eq!("day".parse::<Recurrence>(), Ok(Recurrence::Daily));
    assert_eq!("Weekly".parse::<Recurrence>(), Ok(Recurrence::Weekly));
    assert_eq!("month".parse::<Recurrence>(), Ok(Recurrence::Monthly));
    assert!("fortnight".parse::<Recurrence>().is_err());
}

#[test]
fn next_date() {
    let d = |s| parse_date(s).unwrap();
    assert_eq!(Recurrence::Daily.next(d("2026-12-31")), d("2027-01-01"));
    assert_eq!(Recurrence::Weekly.next(d("2026-10-30")), d("2026-11-06"));
    assert_eq!(Recurrence::Monthly.next(d("2026-10-15")), d("2026-11-15"));
    // Clamped to the last day of a shorter month.
    assert_eq!(Recurrence::Monthly.next(d("2026-01-31")), d("2026-02-28"));
    assert_eq!(Recurrence::Monthly.next(d("2028-01-31")), d("2028-02-29"));
    assert_eq!(Recurrence::Monthly.next(d("2026-12-31")), d("2027-01-31"));
}

#[test]
fn quick_add_with_recurrence() {
    let q = parse_quick_add("Pay rent every:month due:2026-11-01 +home").unwrap();
    assert_eq!(q.recur, Some(Recurrence::Monthly));
    assert_eq!(q.title, "Pay rent");
    let task = Task::from_quick_add(TaskId(1), q, parse_date("2026-10-23").unwrap());
    assert_eq!(task.recur, Some(Recurrence::Monthly));
    assert_eq!(
        task.to_string(),
        "#1 [open] Pay rent +home due:2026-11-01 every:month"
    );
}

#[test]
fn recurrence_needs_a_due_date() {
    assert_eq!(
        parse_quick_add("Stretch every:day"),
        Err(ModelError::RecurrenceWithoutDue)
    );
    assert!(matches!(
        parse_quick_add("x every:hour due:2026-11-01"),
        Err(ModelError::UnknownRecurrence(_))
    ));
}
