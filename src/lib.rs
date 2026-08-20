pub use incr_macros::{Db, query};

pub mod db;
pub mod runtime;

pub use db::{
    AnyMemo, Database, DepId, Epoch, InputField, InputId, InputTable, Memo, MemoTable, QueryId,
};
