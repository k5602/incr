use crate::db::{Database, InputId, QueryId};

pub fn record_input_dep(_id: InputId) {
    // TODO Connects to active query stack
}

pub fn execute_query<DB: Database, V, F>(_db: &DB, _query_id: QueryId, compute: F) -> V
where
    F: FnOnce(&DB) -> V,
{
    // TODO Direct compute for now; connects to MemoTable later
    compute(_db)
}
