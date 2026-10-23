//! `Command::Stats`: a summary of the store.

use chrono::NaiveDate;
use cli::{run, Command};
use store::Store;

fn today() -> NaiveDate {
    model::parse_date("2026-10-23").unwrap()
}

fn exec(store: &mut Store, command: Command) -> String {
    run(store, &command, today()).unwrap()
}

fn add(store: &mut Store, line: &str) {
    exec(store, Command::Add { line: line.into() });
}

#[test]
fn empty_store() {
    let mut s = Store::new();
    assert_eq!(exec(&mut s, Command::Stats), "no tasks");
}

#[test]
fn summary_counts_statuses_priorities_tags_and_overdue() {
    let mut s = Store::new();
    add(&mut s, "Write report +work !high due:2026-10-30");
    add(&mut s, "Water plants +home");
    add(&mut s, "Fix brakes +home !urgent due:2026-10-20");
    add(&mut s, "Pay rent +home due:2026-10-01");
    add(&mut s, "Old chore +home !low due:2026-09-01");
    exec(&mut s, Command::Start { id: 1 });
    exec(&mut s, Command::Done { id: 5 });

    assert_eq!(
        exec(&mut s, Command::Stats),
        "5 tasks: 3 open, 1 in progress, 1 done\n\
         overdue: 2\n\
         priority: urgent 1, high 1, normal 2, low 1\n\
         tags: home 4, work 1"
    );
}

#[test]
fn tags_are_sorted_by_count_then_name_and_untagged_tasks_are_skipped() {
    let mut s = Store::new();
    add(&mut s, "a +zeta");
    add(&mut s, "b +alpha");
    add(&mut s, "c +zeta +alpha");
    add(&mut s, "d +beta");
    add(&mut s, "e");
    assert_eq!(
        exec(&mut s, Command::Stats).lines().last(),
        Some("tags: alpha 2, zeta 2, beta 1")
    );
}

#[test]
fn no_tags_line_when_nothing_is_tagged() {
    let mut s = Store::new();
    add(&mut s, "plain task");
    assert_eq!(
        exec(&mut s, Command::Stats),
        "1 tasks: 1 open, 0 in progress, 0 done\n\
         overdue: 0\n\
         priority: urgent 0, high 0, normal 1, low 0"
    );
}
