use crate::db::{Database, DepId, Epoch, InputId, Memo, MemoTable, QueryId};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Debug;
use std::ops::ControlFlow;

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

/// Cached-tree validation outcome. Open-walk revisits report Dirty;
/// true cycles still panic later through ACTIVE.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    Clean,
    Dirty,
}

/// Validates the cached tree under `root`. Clean nodes are cached, so a
/// shared dependency validates once.
fn dep_tree_verdict(table: &MemoTable, root: &QueryId) -> Verdict {
    fn visit(
        table: &MemoTable,
        id: &QueryId,
        clean: &mut HashSet<QueryId>,
        path: &mut HashSet<QueryId>,
    ) -> Verdict {
        if clean.contains(id) {
            return Verdict::Clean;
        }
        if !path.insert(id.clone()) {
            return Verdict::Dirty;
        }
        let verdict = decide(table, id, clean, path);
        path.remove(id);
        if verdict == Verdict::Clean {
            clean.insert(id.clone());
        }
        verdict
    }

    fn decide(
        table: &MemoTable,
        id: &QueryId,
        clean: &mut HashSet<QueryId>,
        path: &mut HashSet<QueryId>,
    ) -> Verdict {
        let Some(raw) = table.get_memo_raw(id) else {
            return Verdict::Dirty;
        };
        if raw.verified_at() == table.epoch() {
            return Verdict::Clean;
        }
        let verified_at = raw.verified_at();
        let mut queries = Vec::new();
        let inputs_clean = raw.deps().iter().all(|dep| match dep {
            DepId::Input(input) => {
                matches!(table.input_changed_at(input), Some(at) if at <= verified_at)
            }
            DepId::Query(query) => {
                queries.push(query.clone());
                true
            }
        });
        // End the borrow before the walk re-enters the table.
        drop(raw);
        if !inputs_clean {
            return Verdict::Dirty;
        }
        queries
            .into_iter()
            .try_fold((), |(), query| match visit(table, &query, clean, path) {
                Verdict::Clean => ControlFlow::Continue(()),
                Verdict::Dirty => ControlFlow::Break(Verdict::Dirty),
            })
            .break_value()
            .unwrap_or(Verdict::Clean)
    }

    visit(table, root, &mut HashSet::new(), &mut HashSet::new())
}

/// Pre-compute decision. Pure: no frames, no stats.
enum Plan<V> {
    Hit { value: V },
    Reuse { value: V },
    Recompute { previous: Option<Memo<V>> },
}

fn plan_for<V>(table: &MemoTable, id: &QueryId, epoch: Epoch) -> Plan<V>
where
    V: Clone + 'static,
{
    let Some(memo) = table.get_memo::<V>(id) else {
        return Plan::Recompute { previous: None };
    };
    if memo.verified_at == epoch {
        return Plan::Hit {
            value: memo.value.clone(),
        };
    }
    if matches!(dep_tree_verdict(table, id), Verdict::Clean) {
        return Plan::Reuse {
            value: memo.value.clone(),
        };
    }
    Plan::Recompute {
        previous: Some(memo),
    }
}

/// Eq cutoff decision. Pure.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cutoff {
    Unchanged { changed_at: Epoch },
    Changed,
}

fn cutoff<V>(previous: Option<&Memo<V>>, value: &V) -> Cutoff
where
    V: PartialEq,
{
    match previous {
        Some(old) if old.value == *value => Cutoff::Unchanged {
            changed_at: old.changed_at,
        },
        _ => Cutoff::Changed,
    }
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

    // Pure decision, only Recompute falls through.
    let previous = match plan_for(table, &query_id, epoch) {
        Plan::Hit { value } => {
            table.bump_stats(|s| s.hits += 1);
            return value;
        }
        Plan::Reuse { value } => {
            table.update_verified_at(&query_id, epoch);
            table.bump_stats(|s| s.reused += 1);
            return value;
        }
        Plan::Recompute { previous } => previous,
    };

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

    // Eq cutoff: same output means downstream caches remain valid.
    let changed_at = match cutoff(previous.as_ref(), &value) {
        Cutoff::Unchanged { changed_at } => {
            table.bump_stats(|s| s.cutoffs += 1);
            changed_at
        }
        Cutoff::Changed => epoch,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Epoch, InputId, Memo, MemoTable, QueryId};

    struct Marker;
    struct Other;

    fn query_id() -> QueryId {
        QueryId::of::<Marker, u32>(&1, "leaf")
    }

    fn input_id() -> InputId {
        InputId::of::<Other, ()>(&(), "cfg")
    }

    fn memo(value: u32, verified_at: Epoch, changed_at: Epoch) -> Memo<u32> {
        Memo {
            value,
            verified_at,
            changed_at,
            deps: vec![DepId::Input(input_id())],
        }
    }

    #[test]
    fn cutoff_keeps_old_changed_at_on_equal_value() {
        let old = memo(7, Epoch(1), Epoch(1));
        assert_eq!(
            cutoff(Some(&old), &7),
            Cutoff::Unchanged {
                changed_at: Epoch(1)
            }
        );
    }

    #[test]
    fn cutoff_advertises_epoch_on_changed_or_missing_value() {
        let old = memo(7, Epoch(1), Epoch(1));
        assert_eq!(cutoff(Some(&old), &8), Cutoff::Changed);
        let none: Option<Memo<u32>> = None;
        assert_eq!(cutoff(none.as_ref(), &8), Cutoff::Changed);
    }

    #[test]
    fn plan_serves_fresh_memo_as_hit() {
        let table = MemoTable::new();
        table.insert_memo(query_id(), memo(7, Epoch::ZERO, Epoch::ZERO));
        assert!(matches!(
            plan_for::<u32>(&table, &query_id(), Epoch::ZERO),
            Plan::Hit { value } if value == 7
        ));
    }

    #[test]
    fn plan_recomputes_without_memo() {
        let table = MemoTable::new();
        assert!(matches!(
            plan_for::<u32>(&table, &query_id(), Epoch::ZERO),
            Plan::Recompute { previous: None }
        ));
    }

    #[test]
    fn plan_reuses_stale_memo_with_clean_tree() {
        let mut table = MemoTable::new();
        table.insert_memo(query_id(), memo(7, Epoch::ZERO, Epoch::ZERO));
        table.note_input(input_id(), Epoch::ZERO);
        let epoch = table.bump_epoch();
        assert!(matches!(
            plan_for::<u32>(&table, &query_id(), epoch),
            Plan::Reuse { value } if value == 7
        ));
    }

    #[test]
    fn plan_recomputes_stale_memo_with_dirty_tree() {
        let mut table = MemoTable::new();
        table.insert_memo(query_id(), memo(7, Epoch::ZERO, Epoch::ZERO));
        table.note_input(input_id(), Epoch::ZERO);
        let epoch = table.bump_epoch();
        // Input moved after verification: the tree is dirty.
        table.note_input(input_id(), epoch);
        assert!(matches!(
            plan_for::<u32>(&table, &query_id(), epoch),
            Plan::Recompute { previous: Some(_) }
        ));
    }
}
