//! SQLite 层：schema、指纹增量更新、计数。
//! 表与 Python 参考实现同名（files/content/sessions/session_messages/agent_files），
//! 但 FTS 分词器用 trigram（中文必需；选型见 docs/bench-001.md）。

use crate::adapters::ADAPTERS;
use crate::message::SessionRef;
use crate::util;
use rusqlite::{params, Connection};
use std::path::Path;

pub const FTS_TOKENIZER: &str = "trigram case_sensitive 0";

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

/// 连接池：查询线程取用归还，避免每查询 5 次冷开连接（WAL 下 ~2ms/次）。
pub struct Pool {
    path: std::path::PathBuf,
    idle: std::sync::Mutex<Vec<Connection>>,
}

pub struct Pooled<'p> {
    pool: &'p Pool,
    conn: Option<Connection>,
}

impl Pool {
    pub fn new(path: &Path, prewarm: usize) -> Self {
        let mut idle = Vec::new();
        for _ in 0..prewarm {
            if let Ok(c) = open(path) {
                idle.push(c);
            }
        }
        Pool { path: path.to_path_buf(), idle: std::sync::Mutex::new(idle) }
    }

    pub fn get(&self) -> Option<Pooled<'_>> {
        let conn = self.idle.lock().ok()?.pop();
        let conn = match conn {
            Some(c) => Some(c),
            None => open(&self.path).ok(),
        };
        conn.map(|c| Pooled { pool: self, conn: Some(c) })
    }
}

impl std::ops::Deref for Pooled<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl Drop for Pooled<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            if let Ok(mut g) = self.pool.idle.lock() {
                if g.len() < 8 {
                    g.push(c);
                }
            }
        }
    }
}

/// 建表（幂等）。
pub fn ensure_tables(conn: &Connection) -> rusqlite::Result<()> {
    // 文件名索引（MFT 全盘 / 目录遍历降级共用）
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
          id INTEGER PRIMARY KEY,
          path TEXT UNIQUE,
          name TEXT,
          pinyin TEXT DEFAULT '',
          pinyin_init TEXT DEFAULT '',
          size INTEGER,
          mtime REAL,
          is_dir INTEGER DEFAULT 0,
          indexed INTEGER DEFAULT 0);
        CREATE INDEX IF NOT EXISTS files_name ON files(name);
        DROP INDEX IF EXISTS files_norm_path;
        CREATE INDEX IF NOT EXISTS files_norm_path_v2
          ON files(replace(lower(path), '/', '\\'));",
    )?;
    // 内容索引
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS content (
          file_id INTEGER PRIMARY KEY,
          text TEXT);
        CREATE VIRTUAL TABLE IF NOT EXISTS content_fts
          USING fts5(text, content='content', content_rowid=file_id,
                     tokenize='{FTS_TOKENIZER}');
        CREATE TRIGGER IF NOT EXISTS content_ai AFTER INSERT ON content BEGIN
          INSERT INTO content_fts(rowid, text) VALUES (new.file_id, new.text);
        END;
        CREATE TRIGGER IF NOT EXISTS content_ad AFTER DELETE ON content BEGIN
          INSERT INTO content_fts(content_fts, rowid, text)
            VALUES('delete', old.file_id, old.text);
        END;
        CREATE TRIGGER IF NOT EXISTS content_au AFTER UPDATE ON content BEGIN
          INSERT INTO content_fts(content_fts, rowid, text)
            VALUES('delete', old.file_id, old.text);
          INSERT INTO content_fts(rowid, text) VALUES (new.file_id, new.text);
        END;",
    ))?;
    // 会话索引
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS sessions (
          id INTEGER PRIMARY KEY,
          agent TEXT,
          session_id TEXT,
          title TEXT,
          path TEXT,
          cwd TEXT,
          project TEXT,
          mtime REAL,
          size INTEGER,
          UNIQUE(agent, path));
        CREATE TABLE IF NOT EXISTS session_messages (
          id INTEGER PRIMARY KEY,
          session_rowid INTEGER,
          role TEXT,
          ts TEXT,
          text TEXT);
        CREATE VIRTUAL TABLE IF NOT EXISTS session_fts
          USING fts5(text, content='session_messages', content_rowid=id,
                     tokenize='{FTS_TOKENIZER}');
        CREATE TRIGGER IF NOT EXISTS session_messages_ai
        AFTER INSERT ON session_messages BEGIN
          INSERT INTO session_fts(rowid, text) VALUES (new.id, new.text);
        END;
        CREATE TRIGGER IF NOT EXISTS session_messages_ad
        AFTER DELETE ON session_messages BEGIN
          INSERT INTO session_fts(session_fts, rowid, text)
            VALUES('delete', old.id, old.text);
        END;
        CREATE TRIGGER IF NOT EXISTS session_messages_au
        AFTER UPDATE ON session_messages BEGIN
          INSERT INTO session_fts(session_fts, rowid, text)
            VALUES('delete', old.id, old.text);
          INSERT INTO session_fts(rowid, text) VALUES (new.id, new.text);
        END;
        CREATE INDEX IF NOT EXISTS session_messages_session
          ON session_messages(session_rowid);",
    ))?;
    // skills / 记忆
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS agent_files (
          id INTEGER PRIMARY KEY,
          kind TEXT,
          agent TEXT,
          path TEXT UNIQUE,
          name TEXT,
          project TEXT,
          mtime REAL,
          size INTEGER,
          text TEXT);
        CREATE VIRTUAL TABLE IF NOT EXISTS agent_files_fts
          USING fts5(text, content='agent_files', content_rowid=id,
                     tokenize='{FTS_TOKENIZER}');
        CREATE TRIGGER IF NOT EXISTS agent_files_ai
        AFTER INSERT ON agent_files BEGIN
          INSERT INTO agent_files_fts(rowid, text) VALUES (new.id, new.text);
        END;
        CREATE TRIGGER IF NOT EXISTS agent_files_ad
        AFTER DELETE ON agent_files BEGIN
          INSERT INTO agent_files_fts(agent_files_fts, rowid, text)
            VALUES('delete', old.id, old.text);
        END;
        CREATE TRIGGER IF NOT EXISTS agent_files_au
        AFTER UPDATE OF text ON agent_files BEGIN
          INSERT INTO agent_files_fts(agent_files_fts, rowid, text)
            VALUES('delete', old.id, old.text);
          INSERT INTO agent_files_fts(rowid, text) VALUES (new.id, new.text);
        END;",
    ))?;
    Ok(())
}

fn delete_session(conn: &Connection, rowid: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM session_messages WHERE session_rowid=?", params![rowid])?;
    conn.execute("DELETE FROM sessions WHERE id=?", params![rowid])?;
    Ok(())
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct AdapterStats {
    pub found: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
    pub messages: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// 全量 discover + 指纹增量更新会话索引。返回每家统计。
pub fn build_session_index(
    conn: &Connection,
) -> std::collections::BTreeMap<String, AdapterStats> {
    let mut stats = std::collections::BTreeMap::new();
    for ad in ADAPTERS {
        let mut st = AdapterStats::default();
        let refs = std::panic::catch_unwind(ad.discover).unwrap_or_else(|_| Vec::new());
        st.found = refs.len();
        // 每家一个事务：WAL 读者只能看到更新前或提交后的完整会话，
        // 不会命中刚被删除、尚待重插的 session_id。
        let tx = match conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) => {
                st.errors.push(format!("开始索引事务失败: {e}"));
                stats.insert(ad.name.to_string(), st);
                continue;
            }
        };
        let mut existing: std::collections::HashMap<String, (i64, f64, i64)> =
            std::collections::HashMap::new();
        {
            let mut stmt = match tx
                .prepare("SELECT id, path, mtime, size FROM sessions WHERE agent=?")
            {
                Ok(s) => s,
                Err(e) => {
                    st.errors.push(format!("读取旧索引失败: {e}"));
                    stats.insert(ad.name.to_string(), st);
                    continue;
                }
            };
            let rows = stmt.query_map(params![ad.name], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?, r.get::<_, i64>(3)?))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    existing.insert(row.1, (row.0, row.2, row.3));
                }
            }
        }
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for r in &refs {
            let key = r.path.to_string_lossy().into_owned();
            seen.insert(key.clone());
            let cur = existing.get(&key).copied();
            if let Some((_, m, s)) = cur {
                if (m - r.mtime).abs() < 1e-3 && s == r.size as i64 {
                    st.skipped += 1;
                    continue;
                }
            }
            // 先删旧行（含消息），再解析插入；由外层事务保证原子可见。
            if let Some((rowid, _, _)) = cur {
                let _ = delete_session(&tx, rowid);
            }
            match index_one_session(&tx, ad.parse, r) {
                Ok(n) => {
                    st.messages += n;
                    st.updated += 1;
                }
                Err(e) => st.errors.push(format!("{}: {}", r.path.display(), e)),
            }
        }
        for (path, (rowid, _, _)) in &existing {
            if !seen.contains(path) {
                let _ = delete_session(&tx, *rowid);
                st.removed += 1;
            }
        }
        if let Err(e) = tx.commit() {
            st.errors.push(format!("提交索引事务失败: {e}"));
        }
        stats.insert(ad.name.to_string(), st);
    }
    stats
}

fn index_one_session(
    conn: &Connection,
    parse_fn: fn(&Path) -> crate::message::Parsed,
    r: &SessionRef,
) -> rusqlite::Result<usize> {
    let parsed = parse_fn(&r.path);
    let cwd = if !parsed.cwd.is_empty() {
        parsed.cwd
    } else {
        r.cwd.clone()
    };
    let title = parsed
        .title
        .or_else(|| r.title.clone())
        .unwrap_or_else(|| r.session_id.clone());
    let project = util::project_from_cwd(&cwd);
    conn.execute(
        "INSERT OR REPLACE INTO sessions (agent, session_id, title, path, cwd, project, mtime, size)
         VALUES (?,?,?,?,?,?,?,?)",
        params![
            r.agent,
            r.session_id,
            title,
            r.path.to_string_lossy().as_ref(),
            cwd,
            project,
            r.mtime,
            r.size as i64
        ],
    )?;
    let rowid = conn.last_insert_rowid();
    let n = parsed.messages.len();
    {
        let mut stmt = conn.prepare(
            "INSERT INTO session_messages (session_rowid, role, ts, text) VALUES (?,?,?,?)",
        )?;
        for m in &parsed.messages {
            stmt.execute(params![rowid, m.role, m.ts, m.text])?;
        }
    }
    Ok(n)
}

// ---------- 计数 ----------

pub fn session_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0)).unwrap_or(0)
}

pub fn file_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap_or(0)
}

pub fn agent_file_counts(conn: &Connection) -> (i64, i64) {
    let mut skills = 0;
    let mut memories = 0;
    if let Ok(mut stmt) =
        conn.prepare("SELECT kind, count(*) FROM agent_files GROUP BY kind")
    {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            for row in rows.flatten() {
                match row.0.as_str() {
                    "skill" => skills = row.1,
                    "memory" => memories = row.1,
                    _ => {}
                }
            }
        }
    }
    (skills, memories)
}

/// 已索引会话的去重 cwd 列表（agent_files 用它找项目级记忆文件）。
pub fn distinct_cwds(conn: &Connection, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT DISTINCT cwd FROM sessions WHERE cwd != '' LIMIT ?")
    {
        if let Ok(rows) = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}
