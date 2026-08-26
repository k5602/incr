use crate::db::{Database, DepId, InputId, Memo, QueryId};
use std::cell::RefCell;
use std::fmt::Debug;

/// Dependencies observed while one query computation runs.
#[derive(Default)]
struct Frame {
    deps: Vec<DepId>,
}

thread_local! {
    /// Stack of active computations. Frames nest through `execute_query`.
    static STACK: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

pub fn record_input_dep(id: InputId) {
    STACK.with(|stack| {
        if let Some(frame) = stack.borrow_mut().last_mut() {
            frame.deps.push(DepId::Input(id));
        }
    });
}

/// Runs a query with memo hits within the current epoch.
///
/// A memo counts as fresh only when `verified_at == epoch`. Any input change
/// bumps the epoch, so stale memos recompute until dependency validation lands.
pub fn execute_query<DB: Database, V, F>(db: &DB, query_id: QueryId, compute: F) -> V
where
    V: Clone + PartialEq + Debug + 'static,
    F: FnOnce(&DB) -> V,
{
    let table = db.memo_table();
    let epoch = table.epoch();

    // The enclosing query depends on this query even when this call hits a
    // memo, validation will compare changed_at, so the edge matters either way.
    STACK.with(|stack| {
        if let Some(frame) = stack.borrow_mut().last_mut() {
            frame.deps.push(DepId::Query(query_id.clone()));
        }
    });

    STACK.with(|stack| stack.borrow_mut().push(Frame::default()));

    let cached = table
        .get_memo::<V>(&query_id)
        .filter(|memo| memo.verified_at == epoch);
    let cached_hit = cached.is_some();

    let value = match cached {
        Some(memo) => memo.value,
        None => compute(db),
    };

    let deps = STACK
        .with(|stack| stack.borrow_mut().pop())
        .map(|frame| frame.deps);

    // Freshen the stored deps alongside the value; the edges above belong to
    // the epoch that actually recomputed.
    if !cached_hit && let Some(deps) = deps {
        table.insert_memo(
            query_id,
            Memo {
                value: value.clone(),
                verified_at: epoch,
                changed_at: epoch,
                deps,
            },
        );
    }

    value
}
