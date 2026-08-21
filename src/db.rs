use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

/// Revision counter for incremental computation.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Epoch(pub u64);

impl Epoch {
    pub const ZERO: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Identifies an input entity in the dependency graph.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InputId {
    type_id: std::any::TypeId,
    name: &'static str,
    key_debug: String,
}

impl InputId {
    pub fn of<Marker: 'static, K: Debug>(key: &K, name: &'static str) -> Self {
        Self {
            type_id: std::any::TypeId::of::<Marker>(),
            name,
            key_debug: format!("{key:?}"),
        }
    }
}

/// Identifies a query entity in the dependency graph.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct QueryId {
    type_id: std::any::TypeId,
    name: &'static str,
    key_debug: String,
}

impl QueryId {
    pub fn of<Marker: 'static, K: Debug>(key: &K, name: &'static str) -> Self {
        Self {
            type_id: std::any::TypeId::of::<Marker>(),
            name,
            key_debug: format!("{key:?}"),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// A dependency edge to an input or query.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DepId {
    Input(InputId),
    Query(QueryId),
}

/// Map-based input storage with per-key revision tracking.
#[derive(Clone, Debug, Default)]
pub struct InputTable<K, V> {
    data: HashMap<K, (V, Epoch)>,
}

impl<K: Hash + Eq + Clone, V: Clone + PartialEq> InputTable<K, V> {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.data.get(key).map(|(v, _)| v)
    }

    pub fn get_with_epoch(&self, key: &K) -> Option<(&V, Epoch)> {
        self.data.get(key).map(|(v, e)| (v, *e))
    }

    pub fn changed_at(&self, key: &K) -> Option<Epoch> {
        self.data.get(key).map(|(_, e)| *e)
    }

    pub fn set(&mut self, key: K, value: V, epoch: Epoch) {
        if let Some((old_val, old_epoch)) = self.data.get_mut(&key) {
            if *old_val == value {
                // Cutoff: value did not change, preserve old changed_at epoch
                return;
            }
            *old_val = value;
            *old_epoch = epoch;
        } else {
            self.data.insert(key, (value, epoch));
        }
    }
}

/// Scalar input storage with revision tracking.
#[derive(Clone, Debug, Default)]
pub struct InputField<V> {
    data: Option<(V, Epoch)>,
}

impl<V: Clone + PartialEq> InputField<V> {
    pub fn new() -> Self {
        Self { data: None }
    }

    pub fn get(&self) -> Option<&V> {
        self.data.as_ref().map(|(v, _)| v)
    }

    pub fn get_with_epoch(&self) -> Option<(&V, Epoch)> {
        self.data.as_ref().map(|(v, e)| (v, *e))
    }

    pub fn changed_at(&self) -> Option<Epoch> {
        self.data.as_ref().map(|(_, e)| *e)
    }

    pub fn set(&mut self, value: V, epoch: Epoch) {
        if let Some((old_val, old_epoch)) = &mut self.data {
            if *old_val == value {
                // Cutoff: value did not change, preserve old changed_at epoch
                return;
            }
            *old_val = value;
            *old_epoch = epoch;
        } else {
            self.data = Some((value, epoch));
        }
    }
}

/// Stored computation memo.
#[derive(Clone, Debug)]
pub struct Memo<V> {
    pub value: V,
    pub verified_at: Epoch,
    pub changed_at: Epoch,
    pub deps: Vec<DepId>,
}

/// Type-erased interface for query memo storage.
pub trait AnyMemo: Debug {
    fn verified_at(&self) -> Epoch;
    fn changed_at(&self) -> Epoch;
    fn deps(&self) -> &[DepId];
    fn set_verified_at(&mut self, epoch: Epoch);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<V: Clone + PartialEq + Debug + 'static> AnyMemo for Memo<V> {
    fn verified_at(&self) -> Epoch {
        self.verified_at
    }

    fn changed_at(&self) -> Epoch {
        self.changed_at
    }

    fn deps(&self) -> &[DepId] {
        &self.deps
    }

    fn set_verified_at(&mut self, epoch: Epoch) {
        self.verified_at = epoch;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Database trait required for incremental storage.
pub trait Database {
    fn memo_table(&self) -> &MemoTable;
    fn memo_table_mut(&mut self) -> &mut MemoTable;
}

/// Central memoization and revision table.
#[derive(Default, Debug)]
pub struct MemoTable {
    epoch: Epoch,
    memos: RefCell<HashMap<QueryId, Box<dyn AnyMemo>>>,
}

impl MemoTable {
    pub fn new() -> Self {
        Self {
            epoch: Epoch::ZERO,
            memos: RefCell::new(HashMap::new()),
        }
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn bump_epoch(&mut self) -> Epoch {
        self.epoch = self.epoch.next();
        self.epoch
    }

    pub fn get_memo_raw(&self, id: &QueryId) -> Option<std::cell::Ref<'_, Box<dyn AnyMemo>>> {
        let borrowed = self.memos.borrow();
        if borrowed.contains_key(id) {
            Some(std::cell::Ref::map(borrowed, |m| m.get(id).unwrap()))
        } else {
            None
        }
    }

    pub fn get_memo<V: Clone + 'static>(&self, id: &QueryId) -> Option<Memo<V>> {
        let borrowed = self.memos.borrow();
        let any_memo = borrowed.get(id)?;
        let memo = any_memo.as_any().downcast_ref::<Memo<V>>()?;
        Some(memo.clone())
    }

    pub fn insert_memo<V: Clone + PartialEq + Debug + 'static>(&self, id: QueryId, memo: Memo<V>) {
        self.memos.borrow_mut().insert(id, Box::new(memo));
    }

    pub fn update_verified_at(&self, id: &QueryId, epoch: Epoch) -> bool {
        let mut borrowed = self.memos.borrow_mut();
        if let Some(memo) = borrowed.get_mut(id) {
            memo.set_verified_at(epoch);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestMarker;

    #[test]
    fn test_epoch_operations() {
        let mut epoch = Epoch::ZERO;
        assert_eq!(epoch.0, 0);

        epoch = epoch.next();
        assert_eq!(epoch.0, 1);

        let next_epoch = epoch.next();
        assert!(next_epoch > epoch);
    }

    #[test]
    fn test_input_table_cutoff() {
        let mut table = InputTable::<u32, String>::new();

        table.set(1, "hello".to_string(), Epoch(1));
        assert_eq!(table.get(&1), Some(&"hello".to_string()));
        assert_eq!(table.changed_at(&1), Some(Epoch(1)));

        // Cutoff: setting same value preserves changed_at
        table.set(1, "hello".to_string(), Epoch(2));
        assert_eq!(table.changed_at(&1), Some(Epoch(1)));

        // New value: changed_at updates to new epoch
        table.set(1, "world".to_string(), Epoch(3));
        assert_eq!(table.changed_at(&1), Some(Epoch(3)));
    }

    #[test]
    fn test_input_field_cutoff() {
        let mut field = InputField::<String>::new();

        field.set("alpha".to_string(), Epoch(1));
        assert_eq!(field.get(), Some(&"alpha".to_string()));
        assert_eq!(field.changed_at(), Some(Epoch(1)));

        // Cutoff: setting same value preserves changed_at
        field.set("alpha".to_string(), Epoch(2));
        assert_eq!(field.changed_at(), Some(Epoch(1)));

        // New value: changed_at updates
        field.set("beta".to_string(), Epoch(3));
        assert_eq!(field.changed_at(), Some(Epoch(3)));
    }

    #[test]
    fn test_memo_table_storage() {
        let memo_table = MemoTable::new();
        let query_id = QueryId::of::<TestMarker, u32>(&42, "test_query");

        assert_eq!(memo_table.epoch(), Epoch::ZERO);

        let memo = Memo {
            value: vec![1, 2, 3],
            verified_at: Epoch(1),
            changed_at: Epoch(1),
            deps: Vec::new(),
        };

        memo_table.insert_memo(query_id.clone(), memo);

        let retrieved: Option<Memo<Vec<i32>>> = memo_table.get_memo(&query_id);
        assert!(retrieved.is_some());
        let unwrapped = retrieved.unwrap();
        assert_eq!(unwrapped.value, vec![1, 2, 3]);
        assert_eq!(unwrapped.verified_at, Epoch(1));

        assert!(memo_table.update_verified_at(&query_id, Epoch(2)));
        let updated: Memo<Vec<i32>> = memo_table.get_memo(&query_id).unwrap();
        assert_eq!(updated.verified_at, Epoch(2));
        assert_eq!(updated.changed_at, Epoch(1));
    }
}
