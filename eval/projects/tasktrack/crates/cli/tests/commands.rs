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

fn list(store: &mut Store, query: &str, sort: &str) -> String {
    exec(
        store,
        Command::List {
            query: query.into(),
            sort: sort.into(),
        },
    )
}

fn seeded() -> Store {
    let mut s = Store::new();
    add(&mut s, "Write report +work !high due:2026-10-30");
    add(&mut s, "Water plants +home");
    add(&mut s, "Fix brakes +home !urgent due:2026-10-20");
    s
}

#[test]
fn add_reports_the_new_id() {
    let mut s = Store::new();
    assert_eq!(
        exec(
            &mut s,
            Command::Add {
                line: "first".into()
            }
        ),
        "added #1"
    );
}

#[test]
fn list_renders_an_aligned_table() {
    let mut s = seeded();
    assert_eq!(
        list(&mut s, "", "id"),
        "#1  open  high    2026-10-30 (in 7d)    Write report +work\n\
         #2  open                                Water plants +home\n\
         #3  open  urgent  2026-10-20 (3d late)  Fix brakes +home"
    );
}

#[test]
fn list_filters_and_sorts() {
    let mut s = seeded();
    let out = list(&mut s, "+home", "priority");
    let ids: Vec<&str> = out.lines().map(|l| &l[..2]).collect();
    assert_eq!(ids, ["#3", "#2"]);
    assert_eq!(list(&mut s, "+garden", "id"), "no tasks");
}

#[test]
fn status_changes() {
    let mut s = seeded();
    assert_eq!(exec(&mut s, Command::Start { id: 2 }), "started #2");
    assert!(run(&mut s, &Command::Start { id: 2 }, today()).is_err());
    assert_eq!(exec(&mut s, Command::Done { id: 2 }), "done #2");
    assert_eq!(exec(&mut s, Command::Purge), "purged 1 finished task(s)");
    assert!(run(&mut s, &Command::Done { id: 2 }, today()).is_err());
}

#[test]
fn remove() {
    let mut s = seeded();
    assert_eq!(
        exec(&mut s, Command::Remove { id: 1 }),
        "removed #1 Write report"
    );
    assert_eq!(s.len(), 2);
}

#[test]
fn bad_input_is_an_error() {
    let mut s = seeded();
    assert!(run(&mut s, &Command::Add { line: "+x".into() }, today()).is_err());
    assert!(run(
        &mut s,
        &Command::List {
            query: "colour:red".into(),
            sort: "id".into()
        },
        today()
    )
    .is_err());
}
