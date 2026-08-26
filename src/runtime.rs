use crate::db::{Database, DepId, InputId, Memo, MemoTable, QueryId};
use std::cell::RefCell;
use std::collections::HashSet;
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

/// True when every transitive dependency of `root` proves the cached value
/// still equals recomputed output: input edges unchanged since the node was
/// last verified and query edges whose own trees hold too.
fn dep_tree_clean(table: &MemoTable, root: &QueryId) -> bool {
    fn visit(table: &MemoTable, id: &QueryId, seen: &mut HashSet<QueryId>) -> bool {
        // A cycle revisits a node; no memo can be proven clean through one,
        // so treat it as dirty and let compute hit cycle detection.
        if !seen.insert(id.clone()) {
            return false;
        }

        let Some(raw) = table.get_memo_raw(id) else {
            return false;
        };
        if raw.verified_at() == table.epoch() {
            return true;
        }
        let verified_at = raw.verified_at();
        let deps = raw.deps().to_vec();
        drop(raw);

        deps.iter().all(|dep| match dep {
            DepId::Input(input) => {
                matches!(table.input_changed_at(input), Some(at) if at <= verified_at)
            }
            DepId::Query(query) => visit(table, query, seen),
        })
    }

    visit(table, root, &mut HashSet::new())
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

    if let Some(memo) = table.get_memo::<V>(&query_id) {
        if memo.verified_at == epoch {
            return memo.value;
        }
        // Stale by epoch, but clean if every recorded dep proves the value
        // unchanged since verification; reuse without re-running compute.
        if dep_tree_clean(table, &query_id) {
            table.update_verified_at(&query_id, epoch);
            STACK.with(|stack| {
                stack.borrow_mut().pop();
            });
            return memo.value;
        }
    }

    let value = compute(db);

    let deps = STACK
        .with(|stack| stack.borrow_mut().pop())
        .map(|frame| frame.deps);

    if let Some(deps) = deps {
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
