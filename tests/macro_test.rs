use incr::{Db, DepId, InputField, InputId, InputTable, Memo, MemoTable, QueryId, query};
use std::cell::Cell;

thread_local! {
    static COMPUTE_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[query]
fn counted(db: &TestDb) -> usize {
    COMPUTE_COUNT.with(|c| c.set(c.get() + 1));
    db.config().len()
}

#[query]
fn outer(db: &TestDb, id: u32) -> String {
    let words = single_arg(db, id);
    format!("{}:", db.config()) + &words.join("-")
}

#[derive(Default, Db)]
struct TestDb {
    #[input]
    src: InputTable<u32, String>,
    #[input]
    extra: InputTable<String, u32>,
    #[input]
    config: InputField<String>,
    memo: MemoTable,
}

#[query]
fn no_args(db: &TestDb) -> usize {
    db.config().len()
}

#[query]
fn single_arg(db: &TestDb, id: u32) -> Vec<String> {
    db.src(id)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

#[query]
fn multi_args(db: &TestDb, prefix: String, id: u32) -> String {
    format!("{prefix}:{}", db.src(id))
}

#[test]
fn test_macro_expansion_and_methods() {
    let mut db = TestDb::default();

    assert_eq!(db.memo.epoch().0, 0);

    db.set_src(1, "hello world from incr".to_string());
    assert_eq!(db.memo.epoch().0, 1);

    db.set_extra("count".to_string(), 42);
    assert_eq!(db.memo.epoch().0, 2);

    db.set_config("active".to_string());
    assert_eq!(db.memo.epoch().0, 3);

    assert_eq!(db.src(1), "hello world from incr");
    assert_eq!(db.extra("count".to_string()), 42);
    assert_eq!(db.config(), "active");

    // Test method syntax and free functions syntax
    assert_eq!(db.no_args(), 6);
    assert_eq!(no_args(&db), 6);

    assert_eq!(db.single_arg(1), vec!["hello", "world", "from", "incr"]);
    assert_eq!(single_arg(&db, 1), vec!["hello", "world", "from", "incr"]);

    assert_eq!(
        db.multi_args("MSG".to_string(), 1),
        "MSG:hello world from incr"
    );
    assert_eq!(
        multi_args(&db, "MSG".to_string(), 1),
        "MSG:hello world from incr"
    );
}

#[test]
fn test_query_memo_hit_and_epoch_invalidation() {
    let mut db = TestDb::default();
    db.set_config("abc".to_string());

    // First call computes and stores the memo.
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 1);

    // Second call in the same epoch is a memo hit.
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 1);

    // Setting the same value still bumps the epoch (cutoff preserves only
    // changed_at). Without dependency tracking any new epoch recomputes.
    db.set_config("abc".to_string());
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 2);

    // Changed input recomputes with the new value.
    db.set_config("abcd".to_string());
    assert_eq!(counted(&db), 4);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 3);
}

#[test]
fn test_dependency_recording() {
    let mut db = TestDb::default();
    db.set_src(1, "a b".to_string());
    db.set_config("cfg".to_string());

    counted(&db);
    let expected_config = InputId::of::<__IncrInputMarker_TestDb_config, ()>(&(), "config");
    let memo: Memo<usize> = db
        .memo
        .get_memo(&QueryId::of::<__IncrQueryMarker_counted, ()>(
            &(),
            "counted",
        ))
        .unwrap();
    assert_eq!(memo.deps, vec![DepId::Input(expected_config)]);

    outer(&db, 1);
    let inner_id = QueryId::of::<__IncrQueryMarker_single_arg, u32>(&1u32, "single_arg");
    let outer_id = QueryId::of::<__IncrQueryMarker_outer, u32>(&1u32, "outer");

    // Outer read one input directly and one query transitively.
    let outer_memo: Memo<String> = db.memo.get_memo(&outer_id).unwrap();
    assert_eq!(
        outer_memo.deps,
        vec![
            DepId::Query(inner_id.clone()),
            DepId::Input(InputId::of::<__IncrInputMarker_TestDb_config, ()>(
                &(),
                "config"
            )),
        ]
    );

    // Inner memo stores its own input edge and keeps epochs sane.
    let inner_memo: Memo<Vec<String>> = db.memo.get_memo(&inner_id).unwrap();
    assert_eq!(
        inner_memo.deps,
        vec![DepId::Input(
            InputId::of::<__IncrInputMarker_TestDb_src, u32>(&1u32, "src")
        )]
    );
    assert_eq!(inner_memo.verified_at, db.memo.epoch());
}
