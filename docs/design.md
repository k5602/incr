# incr design

One file. All the calls for v0.1 are here so I don't have to jump between RFCs "GSOC IS DONE ALREADY".

## Goal

Build a small query-based incremental library in Rust. You give it inputs, you write pure queries, it caches and reuses. Pull based. Caller asks for a value, we compute only what is needed and what changed.

Non-goals for v0.1: push or reactive streams, distributed, on-disk cache, parallelism. Those can come later if the core holds.

## Core model

### Epoch

Single `Epoch(u64)` on `Db`. Starts at 0. Every `set_input` does `epoch += 1` and records `changed_at = epoch` for that input.

No durability levels. Salsa uses them to skip checks for stable inputs. We get most of that for free with early cutoff. Simpler API, same win for many workloads.

### Memo

```rust
struct Memo {
    value: Value,
    verified_at: Epoch,
    changed_at: Epoch,
    deps: Vec<DepId>,
}
enum DepId { Input(InputId), Query(QueryId) }
```

Table is `HashMap<QueryId, Memo>` inside `MemoTable`. For v0.1 it is unbounded and single threaded (`RefCell`). Later we add LRU keyed by `verified_at`.

`QueryId` is `(query_fn_id, Key)` hashed. `Key: Hash + Eq + Clone`, `Value: Clone + Eq + Debug`. We clone on hit. No interning in v0.1.

### Red-green with Eq cutoff

When you call `db.my_query(k)`:

1. If no memo, run it, record deps seen on the thread-local stack, store with `verified_at = changed_at = current_epoch`.
2. If memo exists and `verified_at == current_epoch`, return clone.
3. Else check each dep:
   - if dep is input: has its `changed_at` moved since `verified_at`?
   - if dep is query: recursively ensure it is up to date, then did its `changed_at` move?
   If any dep has a newer `changed_at`, re-run the query. Else mark `verified_at = current_epoch` and return clone without re-running.
4. After re-run, compare new value with old via `==`. If equal, keep old `changed_at`. This is the cutoff. Parents that depend on this query will then think nothing changed and skip their own re-run.

This is the only way we avoid cascading work. No manual hints and works for pure queries.

### Dependency tracking

Thread-local `Vec<QueryId>` as a stack. On query entry push, on exit pop. Any `db.get_input` or nested `db.other_query` inside the current query pushes an edge to the current frame. If we see the same `QueryId` already on the stack we have a cycle.

### Cycles

Panic. We collect the stack and panic with `CycleError { stack }`. This is honest for a pure query graph. Type inference and other fixpoint cases need a fallback value. That is `#[query(cycle = fallback)]` later.

### API shape

Plain struct.

```rust
#[derive(Default)]
struct Db {
    #[input] src: HashMap<u32, String>,
    memo: incr::MemoTable,
}

#[query]
fn parse(db: &Db, id: u32) -> Vec<String> { ... db.src(id) ... }

#[query]
fn count(db: &Db, id: u32) -> usize { db.parse(id).len() }
```

`#[input]` generates `set_src(&mut self, k, v)` that inserts and bumps epoch, and `src(&self, k) -> V` that clones the input and records a dep if inside a query. `#[query]` generates a wrapper that does the memo logic and calls the user function only on miss or changed deps.

`set_*` needs `&mut self`. That makes the writer exclusive by Rust rules. Queries take `&self`. No `RwLock` needed for single thread.

If we are not inside a query, `src(&self, k)` just returns without recording. That keeps non-query code simple.

### Threads

Single threaded for v0.1. `MemoTable` is `RefCell<HashMap<_,_>>`. Values do not need `Send`. Later we swap to `RwLock` or `DashMap` and add:

```rust
impl Db { fn snapshot(&self) -> Snapshot { ... } }
```

Snapshot clones `Arc<MemoTable>` and `Epoch` so readers can run in parallel while a writer waits for snapshots to drop. At that point `Value: Send + Sync` will be required.

### Persistence

None. Memo is kept in memory and dropped with `Db`. We do keep `Memo` fields serde-friendly so a future `persist` feature can add `db.save(path)` and `db.load(path)` with a single file. No code for it now.

### Purity contract

Queries must be pure. Same inputs and same deps must give same output, no I/O, no interior mutation, no rng. We cannot enforce this with the type system, we document it and test it. A debug helper can re-run a query from scratch and assert `==` to catch accidental impurity.

From-scratch consistency is the guarantee we test: any sequence of `set_input` and `query` calls returns the same as if we built a fresh `Db` and replayed the sets then ran the queries once.

## What we build first

Order matters. I will do it in this order:

1. `incr-macros` crate - `#[input]` and `#[query]` proc macros, second time around proc macros so any help or feedback is much appreciated.
2. `src/db.rs` - `Epoch`, `InputTable`, `MemoTable`, thread-local stack.
3. `src/lib.rs` - re-exports, `CycleError`, `stats()`.

## Observability

Small. `db.stats() -> Stats { hits, misses, recomputes, cutoffs }`. And `CycleError` prints the stack with `Debug`. No dot export or tracing yet. We add those when we actually need to debug a slow case.

## Links I used

Just the papers and posts I actually read : Salsa algorithm notes, rust-analyzer durable incrementality writeup, Adapton and two self-adjusting computation papers, demanded summarization. They are in my browser history if anyone is interested we can chat about it.
