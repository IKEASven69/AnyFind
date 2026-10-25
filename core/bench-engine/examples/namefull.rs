use anyfind_core::{db, dsl, name_index::NameIndex};
use std::time::Instant;

fn main() {
    let dbp = std::env::temp_dir().join("ff-full.db");
    let conn = db::open(&dbp).expect("open ff-full.db");
    let t0 = Instant::now();
    let idx = NameIndex::load_from_db(&conn).unwrap();
    println!("load {} entries in {:?}", idx.entries.len(), t0.elapsed());
    // 持久化 + 冷启动装载
    let blob = std::env::temp_dir().join("ff-names.bin");
    let t = Instant::now();
    idx.save_blob(&blob).unwrap();
    println!("save blob {:?}", t.elapsed());
    let t = Instant::now();
    let idx2 = NameIndex::load_blob(&blob).unwrap();
    println!("load blob {} entries in {:?}", idx2.entries.len(), t.elapsed());
    for q in ["m", "报告", "index", ".json", "nginx", "spec", "claude"] {
        let dq = dsl::parse(q);
        let mut ts = Vec::new();
        let mut n = 0;
        for _ in 0..20 {
            let t = Instant::now();
            let hits = idx2.search(&dq, 20);
            ts.push(t.elapsed());
            n = hits.len();
        }
        ts.sort();
        println!("{q:8} med={:?} hits(top20)={n}", ts[10]);
    }
}
