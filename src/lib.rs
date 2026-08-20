pub use incr_macros::{Db, query};
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Epoch(pub u64);

pub trait Database {
    fn memo_table(&self) -> &MemoTable;
    fn memo_table_mut(&mut self) -> &mut MemoTable;
}

#[derive(Default, Debug)]
pub struct MemoTable {
    epoch: Epoch,
}

impl MemoTable {
    pub fn new() -> Self {
        Self { epoch: Epoch(0) }
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn bump_epoch(&mut self) -> Epoch {
        self.epoch.0 += 1;
        self.epoch
    }
}

#[derive(Clone, Debug, Default)]
pub struct InputTable<K, V> {
    data: HashMap<K, (V, Epoch)>,
}

impl<K: Hash + Eq + Clone, V: Clone> InputTable<K, V> {
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

    pub fn set(&mut self, key: K, value: V, epoch: Epoch) {
        self.data.insert(key, (value, epoch));
    }
}

#[derive(Clone, Debug, Default)]
pub struct InputField<V> {
    data: Option<(V, Epoch)>,
}

impl<V: Clone> InputField<V> {
    pub fn new() -> Self {
        Self { data: None }
    }

    pub fn get(&self) -> Option<&V> {
        self.data.as_ref().map(|(v, _)| v)
    }

    pub fn set(&mut self, value: V, epoch: Epoch) {
        self.data = Some((value, epoch));
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InputId {
    type_id: std::any::TypeId,
    name: &'static str,
    key_debug: String,
}

impl InputId {
    pub fn of<Marker: 'static, K: std::fmt::Debug>(key: &K, name: &'static str) -> Self {
        Self {
            type_id: std::any::TypeId::of::<Marker>(),
            name,
            key_debug: format!("{key:?}"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct QueryId {
    type_id: std::any::TypeId,
    name: &'static str,
    key_debug: String,
}

impl QueryId {
    pub fn of<Marker: 'static, K: std::fmt::Debug>(key: &K, name: &'static str) -> Self {
        Self {
            type_id: std::any::TypeId::of::<Marker>(),
            name,
            key_debug: format!("{key:?}"),
        }
    }
}

pub mod runtime {
    use super::{Database, InputId, QueryId};

    pub fn record_input_dep(_id: InputId) {
        //TODO Will connect to active query stack
    }

    pub fn execute_query<DB: Database, V, F>(_db: &DB, _query_id: QueryId, compute: F) -> V
    where
        F: FnOnce(&DB) -> V,
    {
        //TODO Direct compute for macro testing; full memoization table connects
        compute(_db)
    }
}
