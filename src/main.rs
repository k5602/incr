use incr::{Db, InputField, InputTable, MemoTable, query};

#[derive(Default, Db)]
struct MyDb {
    #[input]
    src: InputTable<u32, String>,
    #[input]
    config: InputField<String>,
    memo: MemoTable,
}

#[query]
fn parse(db: &MyDb, id: u32) -> Vec<String> {
    db.src(id)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

#[query]
fn count_words(db: &MyDb, id: u32) -> usize {
    db.parse(id).len()
}

fn main() {
    let mut db = MyDb::default();
    db.set_src(1, "Glory Glory Man United".to_string());
    db.set_config("default_mode".to_string());

    println!("Words: {}", db.count_words(1));
    println!("Config: {}", db.config());
}
