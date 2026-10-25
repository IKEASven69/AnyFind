use anyfind_core::{db, resume, search};
use rusqlite::params;

fn kinds() -> Vec<String> {
    search::ALL_KINDS.iter().map(|s| s.to_string()).collect()
}

fn groups(
    pool: &db::Pool,
    query: &str,
    kind: &str,
    offset: usize,
) -> Vec<serde_json::Value> {
    search::unified_search(pool, None, query, &kinds(), 20, offset)
        .groups
        .get(kind)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

#[test]
fn filters_only_browse_all_five_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    let conn = db::open(&db_path).unwrap();
    db::ensure_tables(&conn).unwrap();
    let index_sql: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name='files_norm_path_v2'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert!(index_sql.contains("'\\'"), "规范化路径索引必须保留反斜杠: {index_sql}");

    for (path, name, size, mtime, text) in [
        ("D:/work/alpha.md", "alpha.md", 2048, 100.0, "alpha body"),
        ("D:/work/beta.txt", "beta.txt", 100, 200.0, "beta body"),
        ("D:/other/gamma.pdf", "gamma.pdf", 4096, 300.0, "gamma body"),
    ] {
        conn.execute(
            "INSERT INTO files(path,name,size,mtime,is_dir,indexed) VALUES(?,?,?,?,0,1)",
            params![path, name, size, mtime],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();
        conn.execute("INSERT INTO content(file_id,text) VALUES(?,?)", params![file_id, text])
            .unwrap();
    }
    for (agent, sid, project, mtime) in [
        ("claude-code", "s1", "alpha", 100.0),
        ("codex", "s2", "beta", 200.0),
    ] {
        conn.execute(
            "INSERT INTO sessions(agent,session_id,title,path,cwd,project,mtime,size)
             VALUES(?,?,?,?,?,?,?,10)",
            params![agent, sid, format!("title {sid}"), format!("D:/{sid}.jsonl"), "D:/work", project, mtime],
        )
        .unwrap();
        let rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO session_messages(session_rowid,role,ts,text) VALUES(?,?,?,?)",
            params![rowid, "user", "t1", format!("question {sid}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_messages(session_rowid,role,ts,text) VALUES(?,?,?,?)",
            params![rowid, "assistant", "t2", format!("answer {sid}")],
        )
        .unwrap();
    }
    for (kind, agent, path, name, project, mtime, text) in [
        ("skill", "claude-code", "D:/skills/a/SKILL.md", "a", "alpha", 100.0, "skill A"),
        ("skill", "codex", "D:/skills/b/SKILL.md", "b", "beta", 200.0, "skill B"),
        ("memory", "claude-code", "D:/work/CLAUDE.md", "CLAUDE.md", "alpha", 300.0, "memory A"),
    ] {
        conn.execute(
            "INSERT INTO agent_files(kind,agent,path,name,project,mtime,size,text)
             VALUES(?,?,?,?,?,?,?,?)",
            params![kind, agent, path, name, project, mtime, 10, text],
        )
        .unwrap();
    }
    let pool = db::Pool::new(&db_path, 2);

    let sessions = groups(&pool, "kind:session agent:codex", "session", 0);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], "s2");

    let user_sessions = groups(&pool, "kind:session role:user", "session", 0);
    assert_eq!(user_sessions.len(), 2);

    let skills = groups(&pool, "kind:skill agent:claude-code", "skill", 0);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["title"], "a");

    let memories = groups(&pool, "kind:memory project:alpha", "memory", 0);
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0]["path"], "D:/work/CLAUDE.md");

    let files = groups(&pool, "kind:file ext:md size:>1kb in:D:/work", "file", 0);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["title"], "alpha.md");
    assert!(files[0]["score"].is_null());

    let contents = groups(&pool, "kind:content ext:txt", "content", 0);
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["snippet"], "beta body");

    let page = groups(&pool, "kind:session", "session", 1);
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["session_id"], "s1");

    let empty = search::unified_search(&pool, None, "", &kinds(), 20, 0);
    assert!(empty.groups.values().all(|v| v.as_array().is_none_or(|a| a.is_empty())));
}

#[test]
fn every_session_search_hit_can_be_read_and_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("index.db");
    let conn = db::open(&db_path).unwrap();
    db::ensure_tables(&conn).unwrap();
    conn.execute(
        "INSERT INTO sessions(agent,session_id,title,path,cwd,project,mtime,size)
         VALUES('codex','s-resume','resume me','D:/s.jsonl','D:/work','work',1,10)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_messages(session_rowid,role,ts,text) VALUES(1,'user','t','find this')",
        [],
    )
    .unwrap();
    let pool = db::Pool::new(&db_path, 2);

    for query in ["find this", "kind:session agent:codex"] {
        let hits = groups(&pool, query, "session", 0);
        assert_eq!(hits.len(), 1);
        let sid = hits[0]["session_id"].as_str().unwrap();
        let agent = hits[0]["agent"].as_str().unwrap();
        let meta = resume::find_session(&conn, sid).expect("搜索命中必须能取到元信息");
        assert_eq!(meta.agent, agent);
        assert!(resume::get_session_messages(&conn, sid, 100).is_some());
        assert_eq!(
            resume::resume_command(agent, &meta.session_id, meta.cwd.as_deref()).unwrap(),
            "codex resume s-resume"
        );
    }
}
