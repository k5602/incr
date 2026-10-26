# incr design

One living design note. All current v0.1 calls are here so I don't have to jump between RFCs. "GSOC IS DONE ALREADY".

## Goal

A small pull-based incremental engine in Rust. Inputs in, pure queries, cached and reused. The caller asks for a value. We compute only what is needed and what changed.

It is also a research instrument. as the narrow question: "when does the simplest possible design pay off, and where does it break?" Baseline: one global revision, pure queries, automatic `Eq` cutoff, no durability, persistence, or eviction. Every deferred feature is a hypothesis. The design will change as experiments teach us something.

Non-goals for v0.1: push/reactive streams, distributed, on-disk cache, parallelism.

## Core model

### Revision

Single `Revision(u64)` on `Db`, starting at 0. The baseline strategy, not a claim that one revision always suffices. `set_input` compares old and new. Equal values do not bump `changed_at`, or every direct reader re-runs for a no-op. `set_force` invalidates when the caller wants it.

Early cutoff avoids recomputation after an equal re-run. Durability avoids the validation walk over a stable subgraph. Different costs. One revision does not buy durability. A later experiment measures the cost. Keep revision access behind a small abstraction, so a durability vector or per-input revisions can land later.

### Memo

```rust
struct Memo<V> {
    value: Arc<V>,
    verified_at: Revision,
    changed_at: Revision,
    deps: SmallVec<[DepId; 4]>,
    fingerprint: Option<u128>,
}

enum DepId {
    Input { field: FieldId, key: InputKey },
    Query { fn_id: QueryFnId, key: QueryKey },
}
```

Store `Arc<V>` and return it on a hit. Cloning large values is the hidden tax. Use `Arc::ptr_eq` before a deeper comparison. `DepId` must identify the exact input or query key, not a hash. Store an interned key id or the full key. `InputMap` and the typed query tables own the ids.

`MemoTable` is a registry of typed tables, not one type-erased map:

```rust
struct MemoTable {
    tables: HashMap<QueryFnId, Box<dyn QueryTable>>,
}
```

Each `TypedTable<K, V>` owns a `HashMap<K, Memo<V>>`. No type-erased key hashing, collisions and downcasts stay local to one query, and persistence gets a clean per-query unit. v0.1 is unbounded and single threaded (`RefCell`). Eviction comes later. A naive LRU can lose `changed_at` and wrongly invalidate parents, so it needs tombstones or fingerprints. Measure first.

`Key` must work as a typed table key and a stable dependency identity. `Value` is `'static`, behind `Arc`. `Debug` is a debug feature, not a production bound.

### Red-green with cutoff

On a query call:

1. No memo: run, record deps on the current frame, `verified_at = changed_at = current_rev`.
2. `verified_at == current_rev`: return the cached `Arc<V>`.
3. Validate deps against `verified_at`. An input is newer if `dep.changed_at > memo.verified_at`. A query is brought up to date first, then compared the same way. Nothing newer: set `verified_at = current_rev` and return the cache.
4. Something newer: re-run. Replace the dep list, never append, so a branch that stopped reading an input loses its stale edge.
5. Compare old and new through the cutoff policy. Equal: keep the old `changed_at` and backdate, so parents see no change.

Record an edge on every read, cache hits included. `Cutoff` has three useful implementations: `EqCutoff` (default), `NoCutoff`, `FingerprintCutoff`. Fingerprints are cheaper for large values, but a collision can miss an update. Use a wide hash and document the tradeoff, or `Eq` where correctness is critical.

### Dependency tracking

The stack belongs to `Db`, not the thread. A thread-local stack would record cross-database edges when two `Db`s share a thread. Use `RefCell<Vec<Frame>>`, push on entry, pop with an RAII guard so panics clean up. Reads and nested queries record an edge in the current frame. The same `(query_fn, key)` already on the stack is a cycle.

No threads inside queries. A spawned thread loses the stack and silently drops deps. Same for `rayon` and async runtimes.

### Cycles

Panic for v0.1: collect the stack, `CycleError { stack }`. Honest for a pure acyclic graph. Fixpoint cases like type inference need a fallback value or monotone fixed-point iteration. Salsa's `cycle_fn`/`cycle_initial` is the reference. A fixpoint needs a partial order with finite height. Add both after the toy compiler has a recursive query.

### API shape

Plain struct. Inputs use `InputMap`, not a bare `HashMap`. The map owns per-key revisions and dependency identities.

```rust
#[derive(Default, incr::Db)]
struct Db {
    #[input] src: InputMap<FileId, Arc<str>>,
    memo: incr::MemoTable,
}

#[query]
impl Db {
    fn parse(&self, id: FileId) -> Arc<Ast> { ... self.src(id) ... }

    fn count_words(&self, id: FileId) -> usize { self.parse(id).len() }
}
```

A bare `#[input]` on a field cannot compile. It needs `#[derive(incr::Db)]` or `#[incr::db]` on the struct. `#[query]` on an `impl` block keeps method syntax by rewriting each method into a wrapper plus a private implementation. Free functions also work:

```rust
#[query]
fn parse(db: &Db, id: FileId) -> Arc<Ast> { ... }
```

Then call `parse(&db, id)`. Pick one style. Do not promise `db.parse(id)` unless the macro generates it.

`InputMap::set(&mut self, k, v)` returns whether the value changed and only bumps that key's `changed_at` when it differs. `get` records `(field_id, key)` inside a query, plain read outside. `set_*` takes `&mut self`, so the writer is exclusive by Rust rules. Queries take `&self` and need no `RwLock`.

Use `BTreeMap` or sorted keys, or document deterministic iteration. A query over a `HashMap` input can depend on hash seed and insertion order. That breaks from-scratch consistency and stable hashing for persistence.

### Threads

Single threaded for v0.1: `MemoTable` is `RefCell<...>`, values need not be `Send`. Parallelism is a research chapter. First honest move is snapshots:

```rust
impl Db { fn snapshot(&self) -> Snapshot { ... } }
```

A snapshot clones `Arc<MemoTable>` and the current revision, so readers run while a writer waits for snapshots to drop. Salsa's model, and a real cost. Measure before promising it. Keep the seams: `MemoTable` as a trait or alias, revision access behind an abstraction, `Value: Send + Sync` only under a `parallel` feature.

### Persistence

None for v0.1. Not a small serialization feature. It needs stable query IDs across builds, stable key encoding, a value-type registry, a schema version, input validation at load, and stable AST node IDs so spans do not invalidate everything. `TypeId`, `DefaultHasher`, and session-local counters are not stable identities.

Likely design: a fingerprint-based dep graph plus a value cache, not one serialized `Memo` table. rustc is the case study: stable IDs, 128-bit fingerprints, old and new dep graphs, cache promotion, `no_hash`/`eval_always`.

### Purity contract

Queries must be pure: same inputs and deps, same output. No I/O, interior mutation, rng, or threads. The type system cannot enforce it. Document, test, and provide an escape hatch: an explicit `untracked` read for non-input values. Use it only when the read cannot affect the result or the caller accepts the invalidation risk. A file read inside a query without `untracked` is a correctness bug.

From-scratch consistency: any `set_input`/`query` sequence returns what a fresh `Db` replayed from scratch would. Necessary, not sufficient. It misses unnecessary work, missed cutoff, and dependency leaks, so add work assertions and counters. A debug helper can re-run a query with deps disabled and assert equality. Sample it, don't run every query in production.

## Observability

`db.stats() -> Stats`: hits, misses, validations, recomputes, cutoffs, dependency edges, max stack depth, cache entries. Timing behind a feature. `CycleError` prints the stack with `Debug`. Dot export behind `debug-graph` when a graph gets too big. `tracing` spans only when a specific question needs them. Stats are part of the research API, not an afterthought.

## Links and Readings

Core:

- Salsa book: algorithm, durability, cycles.
- rust-analyzer, "Durable Incrementality".
- rustc dev guide, "Incremental compilation in detail".
- Build Systems a la Carte, for the vocabulary: constructive vs detecting traces, demand-driven vs push, early cutoff, minimality.

Adjacent designs:

- comemo and the Typst post on constrained memoization.
- Jane Street Incremental and the Rust `incremental` port.
- Reactively and Alien Signals, for push-pull signals.
- Differential Dataflow and DBSP, for incremental collections.
- Adapton and Acar's self-adjusting computation, for the foundations.
