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
    /// Query ids currently being computed, innermost last. Only the compute
    /// path touches it; cache hits reuse values and cannot form cycles.
    static ACTIVE: RefCell<Vec<QueryId>> = const { RefCell::new(Vec::new()) };
}

/// Detected dependency cycle. `stack` lists every query from the outermost
/// active computation down to the repeated one.
#[derive(Clone, Debug)]
pub struct CycleError {
    pub stack: Vec<QueryId>,
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cycle detected:")?;
        for id in &self.stack {
            write!(f, " {} ->", id.name())?;
        }
        write!(f, " {}", self.stack[0].name())
    }
}

/// Pops ACTIVE and the computation frame during unwind so a recovered panic
/// cannot leave stale ids or dead frames that would corrupt later queries on
/// the same thread.
struct CycleGuard;

impl Drop for CycleGuard {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.borrow_mut().pop();
        });
        STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
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

/// Runs a query with memoization, dependency validation, and Eq cutoff.
///
/// Early exits happen before any frame is pushed so returns can never leak a
/// frame; a leaked frame would swallow later record_input_dep calls.
pub fn execute_query<DB: Database, V, F>(db: &DB, query_id: QueryId, compute: F) -> V
where
    V: Clone + PartialEq + Debug + 'static,
    F: FnOnce(&DB) -> V,
{
    let table = db.memo_table();
    let epoch = table.epoch();

    // Parent edge registers first: whatever we do next (hit, validate, or
    // recompute), the enclosing query depends on us. Our own frame is not
    // pushed yet, so this lands on the true parent.
    STACK.with(|stack| {
        if let Some(frame) = stack.borrow_mut().last_mut() {
            frame.deps.push(DepId::Query(query_id.clone()));
        }
    });

    let previous = table.get_memo::<V>(&query_id);

    if let Some(memo) = &previous {
        // Fresh within this epoch.
        if memo.verified_at == epoch {
            table.bump_stats(|s| s.hits += 1);
            return memo.value.clone();
        }
        // Stale by epoch, but every recorded dep proves the value unchanged
        // since verification: reuse without re-running compute.
        if dep_tree_clean(table, &query_id) {
            table.update_verified_at(&query_id, epoch);
            table.bump_stats(|s| s.reused += 1);
            return memo.value.clone();
        }
    }

    // Re-entering a query that is still computing means a dependency cycle;
    // panic with the active stack as the trace.
    let cycle_free = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        match active.iter().position(|q| q == &query_id) {
            Some(_) => false,
            None => {
                active.push(query_id.clone());
                true
            }
        }
    });
    if !cycle_free {
        let trace = ACTIVE.with(|active| active.borrow().clone());
        // panic_any keeps the typed payload downcastable by callers.
        std::panic::panic_any(CycleError { stack: trace });
    }

    // Frame exists strictly around compute; inputs and nested queries record
    // into it through record_input_dep and their own registrations. The guard
    // unwinds both stacks when compute panics, so normal-path pops below are
    // safe only because they happen while the guard is still alive: the deps
    // frame is drained with take() first, leaving an empty placeholder for
    // the guard to discard.
    STACK.with(|stack| stack.borrow_mut().push(Frame::default()));
    let guard = CycleGuard;

    table.bump_stats(|s| s.recomputes += 1);
    let value = compute(db);

    let deps = STACK
        .with(|stack| {
            stack
                .borrow_mut()
                .last_mut()
                .map(|frame| std::mem::take(&mut frame.deps))
        })
        .unwrap_or_default();
    drop(guard);

    // Eq cutoff: same output means downstream caches remain valid, so keep
    // the old changed_at instead of advertising a change that did not happen.
    let changed_at = match &previous {
        Some(old) if old.value == value => {
            table.bump_stats(|s| s.cutoffs += 1);
            old.changed_at
        }
        _ => epoch,
    };

    table.insert_memo(
        query_id,
        Memo {
            value: value.clone(),
            verified_at: epoch,
            changed_at,
            deps,
        },
    );

    value
}
