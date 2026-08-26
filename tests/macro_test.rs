use incr::{
    CycleError, Db, DepId, Epoch, InputField, InputId, InputTable, Memo, MemoTable, QueryId, Stats,
    query,
};
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

    // Same-value set bumps the epoch but keeps changed_at; dep validation
    // proves the memo clean, so no recompute.
    db.set_config("abc".to_string());
    assert_eq!(counted(&db), 3);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 1);

    // Changed input recomputes with the new value.
    db.set_config("abcd".to_string());
    assert_eq!(counted(&db), 4);
    assert_eq!(COMPUTE_COUNT.with(Cell::get), 2);
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

#[test]
fn test_transitive_validation_skips_recompute() {
    let mut db = TestDb::default();
    db.set_src(1, "a".to_string());
    db.set_config("c".to_string());

    // Warm both memos.
    outer(&db, 1);
    single_arg(&db, 1);

    // Touch only an input inner does not read.
    db.set_extra("noise".to_string(), 7);

    let outer_id = QueryId::of::<__IncrQueryMarker_outer, u32>(&1u32, "outer");
    let before = COMPUTE_COUNT.with(Cell::get);
    assert_eq!(outer(&db, 1), "c:a");
    assert_eq!(COMPUTE_COUNT.with(Cell::get), before);

    // Outer memo verified in place by the clean-tree walk.
    let outer_memo: Memo<String> = db.memo.get_memo(&outer_id).unwrap();
    assert_eq!(outer_memo.changed_at, Epoch(2));
}

thread_local! {
    static STABLE_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[query]
fn cyc_a(db: &TestDb) -> usize {
    1 + cyc_b(db)
}

#[query]
fn cyc_b(db: &TestDb) -> usize {
    1 + cyc_a(db)
}

#[test]
fn test_cycle_detection_panics_with_trace() {
    let mut db = TestDb::default();
    db.set_config("cfg".to_string());

    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cyc_a(&db)))
        .err()
        .and_then(|payload| payload.downcast_ref::<CycleError>().cloned())
        .expect("cyclic query must panic with CycleError");

    let names: Vec<_> = err.stack.iter().map(|id| id.name()).collect();
    assert_eq!(names, ["cyc_a", "cyc_b"]);
    assert_eq!(err.to_string(), "cycle detected: cyc_a -> cyc_b -> cyc_a");

    // Guard restored both stacks during unwind: same-thread queries work.
    assert_eq!(stable_len(&db), 3);
    assert_eq!(STABLE_COUNT.with(Cell::get), 1);
}

#[query]
fn stable_len(db: &TestDb) -> usize {
    STABLE_COUNT.with(|c| c.set(c.get() + 1));
    db.config().len()
}

#[test]
fn test_eq_cutoff_keeps_changed_at() {
    let mut db = TestDb::default();
    db.set_config("abc".to_string());

    // Epoch 1: first compute.
    stable_len(&db);
    let id = QueryId::of::<__IncrQueryMarker_stable_len, ()>(&(), "stable_len");

    // Different input bytes, same output: compute runs once, but changed_at
    // stays at the epoch of the last real change.
    db.set_config("xyz".to_string());
    assert_eq!(stable_len(&db), 3);
    assert_eq!(STABLE_COUNT.with(Cell::get), 2);

    let memo: Memo<usize> = db.memo.get_memo(&id).unwrap();
    assert_eq!(memo.changed_at, Epoch(1));
    assert_eq!(memo.verified_at, Epoch(2));
}

#[test]
fn test_stats_track_engine_work() {
    let mut db = TestDb::default();
    db.set_config("abc".to_string());

    // Both fresh computes.
    counted(&db);
    stable_len(&db);
    assert_eq!(
        db.memo.stats(),
        Stats {
            hits: 0,
            reused: 0,
            recomputes: 2,
            cutoffs: 0,
        }
    );

    // Same-epoch reruns: pure hits.
    counted(&db);
    stable_len(&db);
    assert_eq!(db.memo.stats().hits, 2);

    // Same-value input set: epoch bumps, validation proves the tree clean,
    // so no user code runs and changed_at stays put.
    db.set_config("abc".to_string());
    counted(&db);
    assert_eq!(db.memo.stats().reused, 1);
    assert_eq!(db.memo.stats().recomputes, 2);

    // Changed bytes, same length output: recompute runs once, Eq cutoff fires.
    db.set_config("xyz".to_string());
    counted(&db);
    let s = db.memo.stats();
    assert_eq!((s.recomputes, s.cutoffs), (3, 1));
}
