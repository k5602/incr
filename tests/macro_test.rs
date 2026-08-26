use incr::{Db, InputField, InputTable, MemoTable, query};
use std::sync::atomic::{AtomicUsize, Ordering};

static COMPUTE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[query]
fn counted(db: &TestDb) -> usize {
    COMPUTE_COUNT.fetch_add(1, Ordering::SeqCst);
    db.config().len()
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
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 1);

    // Second call in the same epoch is a memo hit.
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 1);

    // Setting the same value still bumps the epoch (cutoff preserves only
    // changed_at). Without dependency tracking any new epoch recomputes.
    db.set_config("abc".to_string());
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 2);

    // Changed input recomputes with the new value.
    db.set_config("abcd".to_string());
    assert_eq!(counted(&db), 4);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 3);
}
