//! A shared dependency must validate once.
use incr::{Db, InputField, MemoTable, query};

#[derive(Default, Db)]
struct DiamondDb {
    #[input]
    config: InputField<String>,
    #[input]
    noise: InputField<String>,
    memo: MemoTable,
}

#[query]
fn leaf(db: &DiamondDb) -> usize {
    db.config().len()
}

#[query]
fn left(db: &DiamondDb) -> usize {
    leaf(db) + 1
}

#[query]
fn right(db: &DiamondDb) -> usize {
    leaf(db) + 2
}

#[query]
fn root(db: &DiamondDb) -> usize {
    left(db) + right(db)
}

#[test]
fn diamond_validates_without_recompute() {
    let mut db = DiamondDb::default();
    db.set_config("abcd".to_string());
    assert_eq!(root(&db), 11);

    // Bump the epoch with an input no query reads.
    db.set_noise("x".to_string());
    assert_eq!(root(&db), 11);

    let s = db.memo.stats();
    assert_eq!((s.recomputes, s.reused, s.hits, s.cutoffs), (4, 1, 1, 0),);
}
