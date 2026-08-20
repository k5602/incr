# incr

A small pull-based incremental computation library for Rust.

You mark some data as inputs and write pure functions as queries. `incr` caches each query result and re-runs only what changed when you update an input. That is the whole idea.

I started this because I like how salsa and rust-analyzer work, mainly intersted in incremental computing aka "self-adjusting computation", but I did not want the weight. Query groups, storage traits, manual durability levels, I wanted something I can read in an afternoon and change without fear. So `incr` starts from zero with a simpler core: one global epoch and automatic early cutoff, no promises but simplicity and long term development.


## How it "will" work/s

```
inputs (you set) -> epoch bump
     |
     v
queries (pure fns, you write) -> memo table { value, verified_at, changed_at, deps }
     |
     v
caller asks db.query(k) -> if deps unchanged, return cached clone; else recompute
```

- One `Epoch(u64)` counter. Every `set_input` bumps it.
- Each memo stores `verified_at` and `changed_at`. On read we check deps. If a dep's value is still `==` old value, we keep `changed_at` and stop. Parents then skip recompute. No durability annotations. Equality does the work.
- Dependency tracking is thread-local. When a query runs we push its id on a stack. Any `get_input` or nested query records an edge.

That is enough for compiler-like maybe IDE workloads where you have many queries and few input changes.

## What I decided

These came out of a short design. All for v0.1:

- workload: generic pull engine, not dataflow or push based
- revision: single `Epoch` plus `Eq` cutoff, no manual durability
- API: plain struct `Db` with `#[input]` on fields and `#[query]` on free functions. No trait, no query groups. Concrete type.
- storage: `HashMap<QueryId, Memo>` with `Vec<DepId>` per entry. Unbounded for now, LRU later.
- cycles: detect via stack, panic with the cycle trace. Fallback values later.
- threads: single threaded for v0.1 (`RefCell` inside). Snapshot parallelism later so `Value` does not need `Send` yet.
- bounds: `Key: Hash + Eq + Clone`, `Value: Clone + Eq + Debug`. No interning yet.
- persistence: none. Memo entries are kept serde-ready so we can add a `persist` feature later.
- correctness: queries must be pure and deterministic. From-scratch consistency is the contract.

If you want the full trace, see `docs/design.md`. It is one file on purpose.

## Quick look

```rust
use incr::{Db, input, query};
use std::collections::HashMap;

#[derive(Default)]
struct MyDb {
    #[input]
    src: HashMap<u32, String>,
    memo: incr::MemoTable,
}

#[query]
fn parse(db: &MyDb, id: u32) -> Vec<String> {
    db.src(id).split_whitespace().map(|s| s.to_string()).collect()
}

#[query]
fn count_words(db: &MyDb, id: u32) -> usize {
    db.parse(id).len()
}

fn main() {
    let mut db = MyDb::default();
    db.set_src(1, "hello world".to_string());
    assert_eq!(db.count_words(1), 2); // computes and caches

    db.set_src(1, "hello world".to_string());
    assert_eq!(db.count_words(1), 2); // cutoff: src changed_at stays, count_words not recomputed

    db.set_src(1, "hello incr".to_string());
    assert_eq!(db.count_words(1), 2); // src changed, parse reruns, count_words reruns
}
```

Actual macro names and field layout will settle once `incr-macros` lands. The shape above is the target.

## Status

Pre-alpha.

Planned v0.1 does not try to be fast for 100k queries. It tries to be correct and easy to read. Performance and features work comes after the tests pass.

## Build and test

```bash
cargo fmt
cargo clippy
cargo test
cargo bench
```

Tests for v0.1:

- `consistency` - random set/query sequences compared to cold recompute
- `cutoff` - same value does not bump `changed_at`
- `cycle` - direct and transitive cycle panics
- `revision_bump` bench - 10k queries, one input change

I don't know how much it will take to grow into a full-featured library, but I'm excited to see where it goes.
## License

TBD. Probably BSD-3 or Apache-2.0.
