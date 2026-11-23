//! 选型实验：rusqlite FTS5 (trigram) vs Tantivy，真实语料基准。
//! 语料 A：六家 harness 会话消息（anyfind-core 适配器现采）。
//! 语料 B：文件名（前身内核遗留 index.db 的 files 表抽 10 万条，真实全盘数据）。
//! 输出 JSON 到 stdout；结论人工写进 docs/bench-001.md。

use rusqlite::params;
use std::time::Instant;

#[derive(serde::Serialize)]
struct QueryStat {
    label: String,
    query: String,
    runs: usize,
    median_us: u128,
    p95_us: u128,
    hits: usize,
}

#[derive(serde::Serialize)]
struct EngineStat {
    engine: String,
    docs: usize,
    index_ms: u128,
    index_bytes: u64,
    queries: Vec<QueryStat>,
}

#[derive(serde::Serialize)]
struct Report {
    session_docs: usize,
    session_bytes: usize,
    file_docs: usize,
    fts5_session: EngineStat,
    tantivy_session: EngineStat,
    fts5_names: EngineStat,
    tantivy_names: EngineStat,
}

fn percentile(mut v: Vec<u128>, p: f64) -> u128 {
    if v.is_empty() {
        return 0;
    }
    v.sort();
    let idx = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    for e in walkdir::WalkDir::new(path).into_iter().flatten() {
        if e.file_type().is_file() {
            total += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

const QUERIES: &[(&str, &str)] = &[
    ("单字符-ascii", "a"),
    ("单字符-中文", "的"),
    ("常用词-英文", "error"),
    ("常用词-中文", "配置"),
    ("短语-中文", "帮我看看"),
    ("AND-两词", "nginx proxy"),
    ("冷僻词", "xyzzyplugh"),
];

const NAME_QUERIES: &[(&str, &str)] = &[
    ("名单字符", "m"),
    ("名常用词", "index"),
    ("名中文", "报告"),
    ("名扩展名", ".json"),
];

// ---------- FTS5 (trigram) ----------

fn fts5_session(docs: &[(i64, String)]) -> EngineStat {
    let tmp = std::env::temp_dir().join(format!("anyfind-bench-fts5-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let dbp = tmp.join("s.db");
    let mut conn = rusqlite::Connection::open(&dbp).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE msgs (id INTEGER PRIMARY KEY, text TEXT);
         CREATE VIRTUAL TABLE msgs_fts USING fts5(text, content='msgs', content_rowid=id,
           tokenize='trigram case_sensitive 0');",
    )
    .unwrap();

    let t0 = Instant::now();
    {
        let tx = conn.transaction().unwrap();
        let mut ins = tx.prepare("INSERT INTO msgs (id, text) VALUES (?,?)").unwrap();
        let mut fts = tx.prepare("INSERT INTO msgs_fts (rowid, text) VALUES (?,?)").unwrap();
        for (id, text) in docs {
            ins.execute(params![id, text]).unwrap();
            fts.execute(params![id, text]).unwrap();
        }
        drop(ins);
        drop(fts);
        tx.commit().unwrap();
    }
    let index_ms = t0.elapsed().as_millis();
    let index_bytes = dir_size(&tmp);

    let mut queries = Vec::new();
    for (label, q) in QUERIES {
        // <3 字符 → LIKE 兜底（与 anyfind-core search 层一致）
        let like_fallback = q.split_whitespace().any(|t| t.chars().count() < 3);
        let mut times = Vec::new();
        let mut hits = 0usize;
        for _ in 0..30 {
            let t1 = Instant::now();
            if like_fallback {
                let mut stmt = conn.prepare("SELECT count(*) FROM msgs WHERE text LIKE ?").unwrap();
                let like = format!("%{q}%");
                hits = stmt.query_row(params![like], |r| r.get::<_, usize>(0)).unwrap_or(0);
            } else {
                let fts_q = q
                    .split_whitespace()
                    .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut stmt = conn
                    .prepare("SELECT count(*) FROM msgs_fts WHERE msgs_fts MATCH ?")
                    .unwrap();
                hits = stmt.query_row(params![fts_q], |r| r.get::<_, usize>(0)).unwrap_or(0);
            }
            times.push(t1.elapsed().as_micros());
        }
        queries.push(QueryStat {
            label: label.to_string(),
            query: q.to_string(),
            runs: 30,
            median_us: percentile(times.clone(), 0.5),
            p95_us: percentile(times, 0.95),
            hits,
        });
    }
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
    EngineStat { engine: "sqlite-fts5-trigram".into(), docs: docs.len(), index_ms, index_bytes, queries }
}

fn fts5_names(names: &[String]) -> EngineStat {
    let tmp = std::env::temp_dir().join(format!("anyfind-bench-fts5n-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let dbp = tmp.join("n.db");
    let mut conn = rusqlite::Connection::open(&dbp).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE files (id INTEGER PRIMARY KEY, name TEXT);
         CREATE VIRTUAL TABLE files_fts USING fts5(name, content='files', content_rowid=id,
           tokenize='trigram case_sensitive 0');",
    )
    .unwrap();

    let t0 = Instant::now();
    {
        let tx = conn.transaction().unwrap();
        let mut ins = tx.prepare("INSERT INTO files (name) VALUES (?)").unwrap();
        for n in names {
            ins.execute(params![n]).unwrap();
        }
        drop(ins);
        tx.commit().unwrap();
        tx2_insert_fts(&conn);
    }
    fn tx2_insert_fts(conn: &rusqlite::Connection) {
        conn.execute("INSERT INTO files_fts(files_fts) VALUES('rebuild')", []).unwrap();
    }
    let index_ms = t0.elapsed().as_millis();
    let index_bytes = dir_size(&tmp);

    let mut queries = Vec::new();
    for (label, q) in NAME_QUERIES {
        let like_fallback = q.chars().count() < 3;
        let mut times = Vec::new();
        let mut hits = 0usize;
        for _ in 0..30 {
            let t1 = Instant::now();
            if like_fallback {
                let mut stmt = conn.prepare("SELECT count(*) FROM files WHERE name LIKE ?").unwrap();
                hits = stmt.query_row(params![format!("%{q}%")], |r| r.get::<_, usize>(0)).unwrap_or(0);
            } else {
                let mut stmt = conn
                    .prepare("SELECT count(*) FROM files_fts WHERE files_fts MATCH ?")
                    .unwrap();
                hits = stmt.query_row(params![format!("\"{q}\"")], |r| r.get::<_, usize>(0)).unwrap_or(0);
            }
            times.push(t1.elapsed().as_micros());
        }
        queries.push(QueryStat {
            label: label.to_string(),
            query: q.to_string(),
            runs: 30,
            median_us: percentile(times.clone(), 0.5),
            p95_us: percentile(times, 0.95),
            hits,
        });
    }
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
    EngineStat { engine: "sqlite-fts5-trigram".into(), docs: names.len(), index_ms, index_bytes, queries }
}

// ---------- Tantivy ----------

struct TantivyCtx {
    index: tantivy::Index,
    text_field: tantivy::schema::Field,
    dir: std::path::PathBuf,
}

fn tantivy_build(dir_name: &str, docs: &[(i64, String)]) -> (TantivyCtx, u128) {
    use tantivy::collector::Count;
    use tantivy::query::QueryParser;
    use tantivy::schema::*;
    use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
    use tantivy::Index;

    let dir = std::env::temp_dir().join(format!("anyfind-bench-{dir_name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut sb = Schema::builder();
    let indexing = TextFieldIndexing::default()
        .set_tokenizer("ngram13")
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let text_field = sb.add_text_field("text", TextOptions::default().set_indexing_options(indexing));
    let schema = sb.build();
    let index = Index::create_in_dir(&dir, schema).unwrap();
    index.tokenizers().register(
        "ngram13",
        TextAnalyzer::builder(NgramTokenizer::new(1, 3, false).unwrap())
            .filter(LowerCaser)
            .build(),
    );
    let t0 = Instant::now();
    let mut writer = index.writer(100_000_000).unwrap();
    for (_id, text) in docs {
        writer.add_document(tantivy::doc!(text_field => text.clone())).unwrap();
    }
    writer.commit().unwrap();
    drop(writer);
    let index_ms = t0.elapsed().as_millis();
    // 触发 reader 预热
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();
    let qp = QueryParser::for_index(&index, vec![text_field]);
    let _ = searcher.search(&qp.parse_query("warmup").unwrap(), &Count).unwrap();
    (TantivyCtx { index, text_field, dir }, index_ms)
}

fn tantivy_queries(ctx: &TantivyCtx, queries: &[(&str, &str)]) -> (Vec<QueryStat>, u64) {
    use tantivy::collector::Count;
    use tantivy::query::QueryParser;
    let reader = ctx.index.reader().unwrap();
    let searcher = reader.searcher();
    let qp = QueryParser::for_index(&ctx.index, vec![ctx.text_field]);
    let mut out = Vec::new();
    for (label, q) in queries {
        let mut times = Vec::new();
        let mut hits = 0usize;
        for _ in 0..30 {
            let t1 = Instant::now();
            let parsed = qp.parse_query(q).unwrap();
            hits = searcher.search(&parsed, &Count).unwrap();
            times.push(t1.elapsed().as_micros());
        }
        out.push(QueryStat {
            label: label.to_string(),
            query: q.to_string(),
            runs: 30,
            median_us: percentile(times.clone(), 0.5),
            p95_us: percentile(times, 0.95),
            hits,
        });
    }
    let size = dir_size(&ctx.dir);
    (out, size)
}

// ---------- 语料 ----------

fn collect_session_corpus() -> (Vec<(i64, String)>, usize) {
    let mut docs = Vec::new();
    let mut id = 1i64;
    let mut bytes = 0usize;
    for ad in anyfind_core::adapters::ADAPTERS {
        for r in (ad.discover)() {
            let parsed = (ad.parse)(&r.path);
            for m in parsed.messages {
                if m.text.is_empty() {
                    continue;
                }
                bytes += m.text.len();
                docs.push((id, m.text));
                id += 1;
            }
        }
    }
    (docs, bytes)
}

fn collect_name_corpus(limit: usize) -> Vec<String> {
    // 优先 filefind 既有 index.db（真实全盘 ~106 万条）。
    // 该库是 WAL 模式且被运行中的 Python 内核持有——只读直连会失败，
    // 先复制 db+wal 到临时目录再开。
    let appdata = std::env::var_os("APPDATA").map(std::path::PathBuf::from);
    if let Some(legacy) = appdata.map(|a| a.join("filefind").join("index.db")).filter(|p| p.is_file()) {
        let tmp = std::env::temp_dir().join(format!("anyfind-namecorpus-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let copy = tmp.join("index.db");
        if std::fs::copy(&legacy, &copy).is_ok() {
            for suffix in ["-wal", "-shm"] {
                let src = legacy.with_file_name(format!("index.db{suffix}"));
                if src.is_file() {
                    let _ = std::fs::copy(&src, tmp.join(format!("index.db{suffix}")));
                }
            }
            if let Ok(conn) = rusqlite::Connection::open(&copy) {
                if let Ok(mut stmt) =
                    conn.prepare("SELECT name FROM files WHERE name != '' LIMIT ?")
                {
                    if let Ok(rows) = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0)) {
                        let v: Vec<String> = rows.flatten().collect();
                        if !v.is_empty() {
                            return v;
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(r"D:\coding")
        .into_iter()
        .filter_entry(|e| !(e.depth() > 0 && e.file_type().is_dir()
            && matches!(e.file_name().to_str(), Some(".git") | Some("node_modules") | Some("target"))))
        .flatten()
    {
        if let Some(n) = e.file_name().to_str() {
            out.push(n.to_string());
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn main() {
    eprintln!("[bench] 采集会话语料…");
    let (session_docs, session_bytes) = collect_session_corpus();
    eprintln!("[bench] 会话消息 {} 条 / {} 字节", session_docs.len(), session_bytes);

    eprintln!("[bench] 采集文件名语料…");
    let names = collect_name_corpus(100_000);
    eprintln!("[bench] 文件名 {} 条", names.len());

    eprintln!("[bench] FTS5 会话索引…");
    let fts5_session = fts5_session(&session_docs);
    eprintln!("[bench] FTS5 文件名索引…");
    let fts5_names = fts5_names(&names);

    eprintln!("[bench] Tantivy 会话索引…");
    let (ctx_s, idx_ms_s) = tantivy_build("tv-session", &session_docs);
    let (tq_s, size_s) = tantivy_queries(&ctx_s, QUERIES);
    let tantivy_session = EngineStat {
        engine: "tantivy-ngram13".into(),
        docs: session_docs.len(),
        index_ms: idx_ms_s,
        index_bytes: size_s,
        queries: tq_s,
    };

    eprintln!("[bench] Tantivy 文件名索引…");
    let name_docs: Vec<(i64, String)> =
        names.iter().enumerate().map(|(i, n)| (i as i64 + 1, n.clone())).collect();
    let (ctx_n, idx_ms_n) = tantivy_build("tv-names", &name_docs);
    let (tq_n, size_n) = tantivy_queries(&ctx_n, NAME_QUERIES);
    let tantivy_names = EngineStat {
        engine: "tantivy-ngram13".into(),
        docs: name_docs.len(),
        index_ms: idx_ms_n,
        index_bytes: size_n,
        queries: tq_n,
    };

    let _ = std::fs::remove_dir_all(&ctx_s.dir);
    let _ = std::fs::remove_dir_all(&ctx_n.dir);

    let report = Report {
        session_docs: session_docs.len(),
        session_bytes,
        file_docs: names.len(),
        fts5_session,
        tantivy_session,
        fts5_names,
        tantivy_names,
    };
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
