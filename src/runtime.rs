use crate::db::{Database, InputId, Memo, QueryId};
use std::fmt::Debug;

pub fn record_input_dep(_id: InputId) {
    // TODO Connects to active query stack
}

/// Runs a query with memo hits within the current epoch.
pub fn execute_query<DB: Database, V, F>(db: &DB, query_id: QueryId, compute: F) -> V
where
    V: Clone + PartialEq + Debug + 'static,
    F: FnOnce(&DB) -> V,
{
    let table = db.memo_table();
    let epoch = table.epoch();

    if let Some(memo) = table.get_memo::<V>(&query_id)
        && memo.verified_at == epoch
    {
        return memo.value;
    }

    let value = compute(db);
    table.insert_memo(
        query_id,
        Memo {
            value: value.clone(),
            verified_at: epoch,
            changed_at: epoch,
            deps: Vec::new(),
        },
    );
    value
}
