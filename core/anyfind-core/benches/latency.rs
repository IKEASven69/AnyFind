//! criterion 延迟基准（SPEC 延迟预算实测）。
//! 语料：内存合成（50k 文件名 + 2k 会话消息 + 500 skills/记忆 + 内容文档），
//! 覆盖：单字符 / 常用词 / 中文 / 字段过滤 / 组合。
//! 真实语料实测值见 docs/bench-001.md（bench-engine 输出）。

use anyfind_core::{db, dsl, search};
use criterion::{criterion_group, criterion_main, Criterion};
use rusqlite::params;

fn build_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dbp = tmp.path().join("bench.db");
    let conn = db::open(&dbp).unwrap();
    db::ensure_tables(&conn).unwrap();
    let mut conn = conn;

    // 文件名 50k
    {
        let tx = conn.transaction().unwrap();
        let mut stmt = tx.prepare(
            "INSERT INTO files (path, name, pinyin, pinyin_init, size, mtime, is_dir, indexed)
             VALUES (?,?,?,?,?,?,?,1)",
        ).unwrap();
        for i in 0..50_000i64 {
            let (name, py, init) = if i % 10 == 0 {
                (format!("第{i}号报告.md"), "dihaobaogao".to_string(), "dhbg".to_string())
            } else if i % 7 == 0 {
                (format!("nginx-config-{i}.conf"), String::new(), String::new())
            } else {
                (format!("file_{i}.rs"), String::new(), String::new())
            };
            stmt.execute(params![
                format!("D:\\code\\proj{}\\{}", i % 500, name),
                name, py, init, i * 13, 1_760_000_000.0f64, i % 23 == 0
            ]).unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    // 会话消息 2k
    {
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO sessions (agent, session_id, title, path, cwd, project, mtime, size)
             VALUES ('claude-code','bench-sid','基准会话','D:\\s.jsonl','D:\\code\\anyfind','anyfind',1760000000.0,100)",
            [],
        ).unwrap();
        let sid = tx.last_insert_rowid();
        let mut stmt = tx.prepare(
            "INSERT INTO session_messages (session_rowid, role, ts, text) VALUES (?,?,?,?)",
        ).unwrap();
        for i in 0..2_000i64 {
            let text = if i % 5 == 0 {
                format!("第 {i} 条消息：请帮我配置 nginx 反向代理，中文内容基准。")
            } else if i % 5 == 1 {
                format!("message {i}: fix the proxy error in nginx config")
            } else {
                format!("普通消息 {i}：一些无关内容 lorem ipsum dolor sit amet")
            };
            stmt.execute(params![
                sid,
                if i % 2 == 0 { "user" } else { "assistant" },
                "2026-10-01T00:00:00",
                text
            ]).unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    // skills/记忆 500
    {
        let tx = conn.transaction().unwrap();
        let mut stmt = tx.prepare(
            "INSERT INTO agent_files (kind, agent, path, name, project, mtime, size, text)
             VALUES (?,?,?,?,?,?,?,?)",
        ).unwrap();
        for i in 0..500i64 {
            stmt.execute(params![
                if i % 2 == 0 { "skill" } else { "memory" },
                "claude-code",
                format!("C:\\Users\\x\\.claude\\skills\\s{i}\\SKILL.md"),
                format!("SKILL-{i}.md"),
                "anyfind",
                1_760_000_000.0f64,
                1000i64,
                format!("技能 {i}：用于配置与部署 nginx 服务的操作手册。error handling guide.")
            ]).unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    // 内容文档 2k
    {
        let tx = conn.transaction().unwrap();
        let mut stmt = tx.prepare("INSERT INTO content (file_id, text) VALUES (?,?)").unwrap();
        for i in 0..2_000i64 {
            stmt.execute(params![
                i + 1,
                format!("文档 {i}：nginx 配置示例与中文说明。proxy pass upstream error。")
            ]).unwrap();
        }
        drop(stmt);
        tx.commit().unwrap();
    }
    drop(conn);
    (tmp, dbp)
}

fn bench_latency(c: &mut Criterion) {
    let (_tmp, dbp) = build_fixture();
    let conn = db::open(&dbp).unwrap();
    let names = anyfind_core::name_index::NameIndex::load_from_db(&conn).unwrap();
    let pool = db::Pool::new(&dbp, 6);
    let all: Vec<String> = search::ALL_KINDS.iter().map(|s| s.to_string()).collect();

    let mut g = c.benchmark_group("unified_search");
    g.sample_size(50);
    for (label, q) in [
        ("单字符", "m"),
        ("常用词-英文", "nginx"),
        ("中文", "配置"),
        ("中文短语", "\"反向代理\""),
        ("AND-两词", "nginx proxy"),
        ("OR", "nginx OR apache"),
        ("排除", "nginx -error"),
        ("字段过滤", "kind:session agent:claude-code project:anyfind 配置"),
        ("role过滤", "kind:session role:user nginx"),
        ("ext过滤", "kind:file ext:rs file"),
    ] {
        g.bench_function(label, |b| {
            b.iter(|| search::unified_search(&pool, Some(&names), q, &all, 20, 0))
        });
    }
    // 文件名组：内存索引 vs SQL LIKE 对照（验证选型）
    g.bench_function("文件名-内存索引", |b| {
        b.iter(|| search::unified_search(&pool, Some(&names), "kind:file file_4999", &all, 20, 0))
    });
    g.bench_function("文件名-SQL-LIKE", |b| {
        b.iter(|| search::unified_search(&pool, None, "kind:file file_4999", &all, 20, 0))
    });
    g.finish();

    let mut g2 = c.benchmark_group("dsl_parse");
    for (label, q) in [
        ("简单", "nginx proxy"),
        ("全字段", "kind:session agent:codex project:x ext:pdf;docx size:>10mb date:2026-10-01..2026-10-20 in:D:/code role:user \"短 语\" a OR b -c"),
    ] {
        g2.bench_function(label, |b| b.iter(|| dsl::parse(q)));
    }
    g2.finish();
}

criterion_group!(benches, bench_latency);
criterion_main!(benches);
