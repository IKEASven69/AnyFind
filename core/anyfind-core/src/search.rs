//! 统一搜索：DSL 解析 → 五类聚合到契约 groups 结构。
//! 移植自 filefind/server.py 的 unified_search，过滤语义升级为 DSL v1。
//! snippet 高亮：FTS 哨兵（\uE000/\uE001）→ HTML 转义 → <mark>。

use crate::db;
use crate::dsl::{self, DateRange, Filter, SizeRange, TermExpr};
use crate::scanner;
use crate::util::{self, SNIP_CLOSE, SNIP_OPEN};
use rusqlite::{params_from_iter, Connection};
use std::time::Instant;

pub const ALL_KINDS: [&str; 5] = ["file", "content", "session", "skill", "memory"];

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub kind: String,
    pub title: Option<String>,
    pub path: Option<String>,
    pub is_dir: Option<bool>,
    pub snippet: Option<String>,
    pub mtime: Option<String>,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub score: Option<f64>,
}

impl Hit {
    fn new(kind: &str) -> Self {
        Hit {
            kind: kind.to_string(),
            title: None,
            path: None,
            is_dir: None,
            snippet: None,
            mtime: None,
            agent: None,
            project: None,
            session_id: None,
            cwd: None,
            score: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResponse {
    pub q: String,
    pub took_ms: u64,
    pub groups: serde_json::Map<String, serde_json::Value>,
    pub truncated: serde_json::Map<String, serde_json::Value>,
}

fn rank_score(rank: f64) -> f64 {
    ((1.0 / (1.0 - rank)) * 10000.0).round() / 10000.0
}

// ---------- 过滤辅助 ----------

fn match_ext(path: Option<&str>, exts: &[String]) -> bool {
    if exts.is_empty() {
        return true;
    }
    let Some(p) = path else { return false };
    let low = p.to_lowercase();
    exts.iter().any(|e| low.ends_with(&format!(".{e}")))
}

fn in_date(mtime_iso: Option<&str>, dr: Option<&DateRange>) -> bool {
    let Some(dr) = dr else { return true };
    let Some(iso) = mtime_iso else { return false };
    let Ok(dt) = chrono::NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S") else {
        return false;
    };
    let Some(ts) = dt
        .and_local_timezone(chrono::Local)
        .single()
        .map(|t| t.timestamp() as f64)
    else {
        return false;
    };
    if let Some(f) = dr.from {
        if ts < f {
            return false;
        }
    }
    if let Some(t) = dr.to {
        if ts > t {
            return false;
        }
    }
    true
}

fn in_size(size: u64, sr: Option<&SizeRange>) -> bool {
    let Some(sr) = sr else { return true };
    if let Some(min) = sr.min {
        if size < min {
            return false;
        }
    }
    if let Some(max) = sr.max {
        if size > max {
            return false;
        }
    }
    true
}

fn in_prefix(path: Option<&str>, prefixes: &[String]) -> bool {
    if prefixes.is_empty() {
        return true;
    }
    let Some(p) = path else { return false };
    let norm = p.to_lowercase().replace('/', "\\");
    prefixes.iter().any(|pre| {
        let pr = pre.to_lowercase().replace('/', "\\");
        let pr = pr.trim_end_matches('\\').to_string();
        norm == pr || norm.starts_with(&format!("{pr}\\")) || norm.starts_with(&pr)
    })
}

#[derive(Default)]
struct Filters {
    kinds: Option<Vec<String>>,
    agents: Vec<String>,
    projects: Vec<String>,
    exts: Vec<String>,
    size: Option<SizeRange>,
    date: Option<DateRange>,
    prefixes: Vec<String>,
    role: Option<String>,
}

impl Filters {
    fn from_query(q: &dsl::Query) -> Self {
        let mut f = Filters::default();
        for flt in &q.filters {
            match flt {
                Filter::Kind(v) => f.kinds = Some(v.clone()),
                Filter::Agent(v) => f.agents.extend(v.iter().cloned()),
                Filter::Project(v) => f.projects.push(v.clone()),
                Filter::Ext(v) => f.exts.extend(v.iter().cloned()),
                Filter::Size(v) => f.size = Some(v.clone()),
                Filter::Date(v) => f.date = Some(v.clone()),
                Filter::In(v) => f.prefixes.push(v.clone()),
                Filter::Role(v) => f.role = Some(v.clone()),
            }
        }
        f
    }

    fn agent_ok(&self, agent: Option<&str>) -> bool {
        self.agents.is_empty() || self.agents.iter().any(|a| Some(a.as_str()) == agent)
    }
    fn project_ok(&self, project: Option<&str>) -> bool {
        self.projects.is_empty()
            || self
                .projects
                .iter()
                .any(|p| project.is_some_and(|x| x.to_lowercase().contains(&p.to_lowercase())))
    }
}

// ---------- 文件组 ----------

/// 文件名匹配分：精确 1.0 > 前缀 0.9 > 子串 0.75 > 拼音 0.6/0.5 > 路径 0.3。
fn file_score(query: &str, name: &str, path: &str) -> f64 {
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    let stem = std::path::Path::new(&n)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| n.clone());
    if n == q || stem == q {
        return 1.0;
    }
    if n.starts_with(&q) || stem.starts_with(&q) {
        return 0.9;
    }
    if n.contains(&q) {
        return 0.75;
    }
    if q.is_ascii() && q.chars().all(|c| c.is_ascii_alphabetic()) {
        let (py, init) = scanner::pinyin_of(name);
        if py.starts_with(&q) || init.starts_with(&q) {
            return 0.6;
        }
        if py.contains(&q) {
            return 0.5;
        }
    }
    if path.to_lowercase().contains(&q) {
        return 0.3;
    }
    0.1
}

/// path 落在某已索引会话的 cwd 下 → 项目名。缓存 5 分钟。
/// 跳过 home 目录与盘根（否则一切文件都归属用户名这类噪声项目）。
fn project_for_path(conn: &Connection, path: &str) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<(f64, Vec<String>)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new((0.0, Vec::new())));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let home = crate::paths::home()
        .to_string_lossy()
        .to_lowercase()
        .replace('/', "\\");
    let cwds = {
        let mut guard = cache.lock().ok()?;
        if now - guard.0 > 300.0 {
            let mut set: Vec<String> = db::distinct_cwds(conn, 1000)
                .into_iter()
                .map(|c| {
                    c.to_lowercase()
                        .replace('/', "\\")
                        .trim_end_matches('\\')
                        .to_string()
                })
                .filter(|c| c.len() > 3 && *c != home)
                .collect();
            set.sort_by_key(|c| std::cmp::Reverse(c.len()));
            set.dedup();
            *guard = (now, set);
        }
        guard.1.clone()
    };
    let p = path.to_lowercase().replace('/', "\\");
    for nc in cwds {
        if p == nc || p.starts_with(&format!("{nc}\\")) {
            return util::project_from_cwd(&nc);
        }
    }
    None
}

struct FileCand {
    hit: scanner::NameHit,
    score: f64,
}

/// 内存索引路径：名字匹配 → rowid 回查 files 表补 path/mtime → 字段过滤。
fn search_file_group_mem(
    conn: &Connection,
    names: &crate::name_index::NameIndex,
    query: &dsl::Query,
    filters: &Filters,
    fetch: usize,
) -> Vec<FileCand> {
    let mut out: Vec<FileCand> = Vec::new();
    let mut want = fetch * 4 + 50;
    for _round in 0..5 {
        let matches = names.search(query, want);
        if matches.is_empty() {
            break;
        }
        // 批量回查 path/mtime/name
        let mut stmt =
            match conn.prepare("SELECT path, name, size, mtime, is_dir FROM files WHERE id=?") {
                Ok(s) => s,
                Err(_) => break,
            };
        out.clear();
        for m in &matches {
            let row = stmt
                .query_row(rusqlite::params![m.rowid], |r| {
                    Ok(scanner::NameHit {
                        path: r.get::<_, String>(0)?,
                        name: r.get::<_, String>(1)?,
                        size: r.get::<_, i64>(2)?,
                        mtime: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                        is_dir: r.get::<_, i64>(4)? != 0,
                    })
                })
                .ok();
            let Some(nh) = row else { continue };
            if !match_ext(Some(&nh.path), &filters.exts) {
                continue;
            }
            if !in_prefix(Some(&nh.path), &filters.prefixes) {
                continue;
            }
            if !in_size(nh.size as u64, filters.size.as_ref()) {
                continue;
            }
            let mtime_iso = util::iso_ts(Some(nh.mtime));
            if !in_date(mtime_iso.as_deref(), filters.date.as_ref()) {
                continue;
            }
            out.push(FileCand {
                hit: nh,
                score: m.score,
            });
            if out.len() >= fetch {
                break;
            }
        }
        if out.len() >= fetch || matches.len() < want {
            break;
        }
        want *= 4; // 过滤掉太多，扩大候选池再试
    }
    out.truncate(fetch);
    out
}

fn browse_file_group(conn: &Connection, filters: &Filters, fetch: usize) -> Vec<FileCand> {
    let (where_sql, mut params) = sql_metadata_filters("f", filters, true);
    params.push((fetch as i64).into());
    let sql = format!(
        "SELECT f.path, f.name, f.size, f.mtime, f.is_dir
         FROM files f WHERE 1=1{where_sql} LIMIT ?"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(FileCand {
            hit: scanner::NameHit {
                path: r.get(0)?,
                name: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                is_dir: r.get::<_, i64>(4)? != 0,
            },
            score: 0.0,
        })
    });
    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn search_file_group(
    conn: &Connection,
    names: Option<&crate::name_index::NameIndex>,
    query: &dsl::Query,
    filters: &Filters,
    fetch: usize,
) -> Vec<FileCand> {
    if query.expr.is_none() {
        return browse_file_group(conn, filters, fetch);
    }
    if let Some(ni) = names {
        if !ni.entries.is_empty() {
            return search_file_group_mem(conn, ni, query, filters, fetch);
        }
    }
    let Some(expr) = &query.expr else {
        return Vec::new();
    };
    let words = dsl::positive_terms(query);
    if words.is_empty() {
        return Vec::new();
    }
    let mut by_path: std::collections::HashMap<String, scanner::NameHit> =
        std::collections::HashMap::new();
    for w in &words {
        for h in scanner::search_name(conn, w, fetch * 3 + 50) {
            by_path.entry(h.path.clone()).or_insert(h);
        }
        if by_path.len() > fetch * 20 + 1000 {
            break; // 防爆
        }
    }
    let mut out: Vec<FileCand> = Vec::new();
    for nh in by_path.into_values() {
        let combined = format!("{}\n{}", nh.name.to_lowercase(), nh.path.to_lowercase());
        if !dsl::text_matches(expr, &combined) {
            continue;
        }
        if !match_ext(Some(&nh.path), &filters.exts) {
            continue;
        }
        if !in_prefix(Some(&nh.path), &filters.prefixes) {
            continue;
        }
        if !in_size(nh.size as u64, filters.size.as_ref()) {
            continue;
        }
        let mtime_iso = util::iso_ts(Some(nh.mtime));
        if !in_date(mtime_iso.as_deref(), filters.date.as_ref()) {
            continue;
        }
        let score = words
            .iter()
            .map(|w| file_score(w, &nh.name, &nh.path))
            .fold(0.0f64, f64::max);
        out.push(FileCand { hit: nh, score });
    }
    // 匹配分优先，目录其次，大小兜底
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.hit.is_dir.cmp(&a.hit.is_dir))
            .then_with(|| b.hit.size.cmp(&a.hit.size))
    });
    out.truncate(fetch);
    out
}

// ---------- FTS / LIKE 行拉取 ----------

/// 从一组 SQL 结果里取文本列做 dsl expr 复核。
/// 纯 AND 长词查询跳过复核（FTS trigram 语义已精确）；复核时按需小写化。
fn fetch_rows(
    conn: &Connection,
    sql: &str,
    params: Vec<rusqlite::types::Value>,
    text_col: usize,
    expr: &TermExpr,
    verify: bool,
    cap: usize,
) -> Vec<Vec<rusqlite::types::Value>> {
    let mut out = Vec::new();
    let Ok(mut stmt) = conn.prepare(sql) else {
        return out;
    };
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        let mut cols = Vec::new();
        let mut i = 0;
        while let Ok(v) = r.get::<_, rusqlite::types::Value>(i) {
            cols.push(v);
            i += 1;
        }
        Ok(cols)
    });
    if let Ok(rows) = rows {
        for cols in rows.flatten() {
            if verify {
                let text = match cols.get(text_col) {
                    Some(rusqlite::types::Value::Text(t)) => t.clone(),
                    _ => String::new(),
                };
                if !dsl::text_matches_raw(expr, &text) {
                    continue;
                }
            }
            out.push(cols);
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}

/// LIKE 兜底 where 子句：纯 AND 词 → AND 链（语义由 LIKE 保证，免复核）；
/// 含 OR/NOT → OR 网 + 调用方逐行复核。返回 (片段, 参数, 需复核)。
fn like_where(
    col: &str,
    terms: &[String],
    expr: &TermExpr,
) -> (String, Vec<rusqlite::types::Value>, bool) {
    let need_verify = dsl::expr_has_or_not(expr);
    let joiner = if need_verify { " OR " } else { " AND " };
    let sql = terms
        .iter()
        .map(|_| format!("{col} LIKE ? ESCAPE '\\'"))
        .collect::<Vec<_>>()
        .join(joiner);
    let p = terms
        .iter()
        .map(|t| {
            rusqlite::types::Value::from(format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")))
        })
        .collect();
    (sql, p, need_verify)
}

/// agent / project 过滤下推到 SQL（Python 语义；后过滤仍保留兜底）。
/// 返回 (where 片段, 参数)。col 如 "s.agent"。
fn sql_in_eq(col: &str, vals: &[String]) -> (String, Vec<rusqlite::types::Value>) {
    if vals.is_empty() {
        return (String::new(), Vec::new());
    }
    let ph = vals.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    (
        format!(" AND {col} IN ({ph})"),
        vals.iter().map(|v| v.clone().into()).collect(),
    )
}

fn sql_project_like(col: &str, vals: &[String]) -> (String, Vec<rusqlite::types::Value>) {
    if vals.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut sql = String::from(" AND (");
    let mut p = Vec::new();
    for (i, v) in vals.iter().enumerate() {
        if i > 0 {
            sql.push_str(" OR ");
        }
        sql.push_str(&format!("{col} LIKE ? ESCAPE '\\'"));
        p.push(format!("%{}%", v.replace('%', "\\%").replace('_', "\\_")).into());
    }
    sql.push(')');
    (sql, p)
}

fn sql_like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 无关键词的字段浏览：把路径、扩展名、时间和大小过滤下推到 SQL，
/// 避免为了前几条结果把整表读进 Rust。
fn sql_metadata_filters(
    alias: &str,
    filters: &Filters,
    with_size: bool,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    if let Some(range) = &filters.date {
        if let Some(from) = range.from {
            sql.push_str(&format!(" AND {alias}.mtime >= ?"));
            params.push(from.into());
        }
        if let Some(to) = range.to {
            sql.push_str(&format!(" AND {alias}.mtime <= ?"));
            params.push(to.into());
        }
    }
    if !filters.prefixes.is_empty() {
        sql.push_str(" AND (");
        for (i, prefix) in filters.prefixes.iter().enumerate() {
            if i > 0 {
                sql.push_str(" OR ");
            }
            // 范围条件可使用 files_norm_path 表达式索引；LIKE 会全盘扫 500 万行。
            sql.push_str(&format!(
                "(replace(lower({alias}.path), '/', '\\') >= ?
                  AND replace(lower({alias}.path), '/', '\\') < ?)"
            ));
            let normalized = prefix.to_lowercase().replace('/', "\\");
            params.push(normalized.clone().into());
            params.push(format!("{normalized}\u{10ffff}").into());
        }
        sql.push(')');
    }
    if !filters.exts.is_empty() {
        sql.push_str(" AND (");
        for (i, ext) in filters.exts.iter().enumerate() {
            if i > 0 {
                sql.push_str(" OR ");
            }
            sql.push_str(&format!("lower({alias}.path) LIKE ? ESCAPE '\\'"));
            params.push(format!("%.{}", sql_like_escape(&ext.to_lowercase())).into());
        }
        sql.push(')');
    }
    if with_size {
        if let Some(range) = &filters.size {
            if let Some(min) = range.min {
                sql.push_str(&format!(" AND {alias}.size >= ?"));
                params.push(i64::try_from(min).unwrap_or(i64::MAX).into());
            }
            if let Some(max) = range.max {
                sql.push_str(&format!(" AND {alias}.size <= ?"));
                params.push(i64::try_from(max).unwrap_or(i64::MAX).into());
            }
        }
    }
    (sql, params)
}

fn val_text(v: Option<&rusqlite::types::Value>) -> Option<String> {
    match v {
        Some(rusqlite::types::Value::Text(t)) => Some(t.clone()),
        Some(rusqlite::types::Value::Integer(i)) => Some(i.to_string()),
        Some(rusqlite::types::Value::Real(f)) => Some(f.to_string()),
        _ => None,
    }
}
fn val_f64(v: Option<&rusqlite::types::Value>) -> Option<f64> {
    match v {
        Some(rusqlite::types::Value::Real(f)) => Some(*f),
        Some(rusqlite::types::Value::Integer(i)) => Some(*i as f64),
        _ => None,
    }
}

fn snippet_of(raw: &str, text: &str, terms: &[String]) -> String {
    if !raw.is_empty() {
        util::render_snippet(raw)
    } else {
        util::render_snippet(&util::make_snippet(text, terms, 60))
    }
}

// ---------- 会话组 ----------

fn browse_session_group(
    conn: &Connection,
    filters: &Filters,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    let (agent_sql, mut params) = sql_in_eq("s.agent", &filters.agents);
    let (proj_sql, proj_p) = sql_project_like("s.project", &filters.projects);
    params.extend(proj_p);
    let (meta_sql, meta_p) = sql_metadata_filters("s", filters, false);
    params.extend(meta_p);
    let role_sql = if filters.role.is_some() {
        params.push(filters.role.clone().unwrap_or_default().into());
        " AND EXISTS (SELECT 1 FROM session_messages mr
                      WHERE mr.session_rowid=s.id AND mr.role=?)"
    } else {
        ""
    };
    let extra_sql = format!("{agent_sql}{proj_sql}{meta_sql}{role_sql}");
    params.push((limit as i64).into());
    params.push((offset as i64).into());
    let sql = format!(
        "WITH matching AS (
           SELECT s.id, s.agent, s.session_id, s.title, s.path, s.cwd, s.project, s.mtime,
                  ROW_NUMBER() OVER (
                    PARTITION BY s.agent, s.session_id
                    ORDER BY s.mtime DESC, s.id DESC
                  ) AS rn
           FROM sessions s WHERE 1=1{extra_sql}
         )
         SELECT agent, session_id, title, path, cwd, project, mtime
         FROM matching WHERE rn=1 ORDER BY mtime DESC LIMIT ? OFFSET ?"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        let sid = r.get::<_, String>(1)?;
        let mut hit = Hit::new("session");
        hit.agent = Some(r.get(0)?);
        hit.session_id = Some(sid.clone());
        hit.title = Some(r.get::<_, Option<String>>(2)?.unwrap_or(sid));
        hit.path = r.get(3)?;
        hit.cwd = r.get::<_, Option<String>>(4)?.filter(|s| !s.is_empty());
        hit.project = r.get(5)?;
        hit.mtime = util::iso_ts(r.get(6)?);
        Ok(hit)
    });
    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn search_session_group(
    conn: &Connection,
    query: &dsl::Query,
    filters: &Filters,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    if query.expr.is_none() {
        return browse_session_group(conn, filters, limit, offset);
    }
    let Some(expr) = &query.expr else {
        return Vec::new();
    };
    let terms = dsl::positive_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let open = SNIP_OPEN.to_string();
    let close = SNIP_CLOSE.to_string();
    let cap = (limit + offset) * 8 + 20;
    // 过滤下推 SQL（Python 语义；后过滤仍保留兜底）：agent / project / role
    let (agent_sql, agent_p) = sql_in_eq("s.agent", &filters.agents);
    let (proj_sql, proj_p) = sql_project_like("s.project", &filters.projects);
    let role_sql = if filters.role.is_some() {
        " AND m.role = ?"
    } else {
        ""
    };
    let extra_sql = format!("{agent_sql}{proj_sql}{role_sql}");
    let mut extra_p = agent_p;
    extra_p.extend(proj_p);
    if let Some(r) = &filters.role {
        extra_p.push(r.clone().into());
    }

    let cols: Vec<Vec<rusqlite::types::Value>>;
    if let Some(fts_q) = dsl::fts5_query(query) {
        let sql = format!(
            "SELECT m.id, snippet(session_fts, 0, ?, ?, '…', 24), session_fts.rank,
                    m.text, s.agent, s.session_id, s.title, s.path, s.cwd, s.project, s.mtime
             FROM session_fts
             JOIN session_messages m ON m.id = session_fts.rowid
             JOIN sessions s ON s.id = m.session_rowid
             WHERE session_fts MATCH ?{extra_sql}
             ORDER BY rank LIMIT ?"
        );
        let mut p: Vec<rusqlite::types::Value> =
            vec![open.clone().into(), close.clone().into(), fts_q.into()];
        p.extend(extra_p.clone());
        p.push((cap as i64).into());
        cols = fetch_rows(conn, &sql, p, 3, expr, dsl::expr_needs_verify(expr), cap);
    } else {
        let (likes, mut p, like_verify) = like_where("m.text", &terms, expr);
        p.extend(extra_p.clone());
        p.push((cap as i64).into());
        let sql = format!(
            "SELECT m.id, '', 0.0, m.text,
                    s.agent, s.session_id, s.title, s.path, s.cwd, s.project, s.mtime
             FROM session_messages m
             JOIN sessions s ON s.id = m.session_rowid
             WHERE ({likes}){extra_sql}
             LIMIT ?"
        );
        cols = fetch_rows(conn, &sql, p, 3, expr, like_verify, cap);
    }

    // 按 (agent, session_id) 去重，offset/limit 在去重后做
    let mut hits = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in cols {
        let agent = val_text(c.get(4)).unwrap_or_default();
        let sid = val_text(c.get(5)).unwrap_or_default();
        if !seen.insert((agent.clone(), sid.clone())) {
            continue;
        }
        if !filters.agent_ok(Some(&agent)) {
            continue;
        }
        let project = val_text(c.get(9));
        if !filters.project_ok(project.as_deref()) {
            continue;
        }
        let mtime = util::iso_ts(val_f64(c.get(10)));
        if !in_date(mtime.as_deref(), filters.date.as_ref()) {
            continue;
        }
        let path = val_text(c.get(7));
        if !in_prefix(path.as_deref(), &filters.prefixes) {
            continue;
        }
        let text = val_text(c.get(3)).unwrap_or_default();
        let mut h = Hit::new("session");
        h.title = Some(val_text(c.get(6)).unwrap_or_else(|| sid.clone()));
        h.path = path;
        h.snippet = Some(snippet_of(
            &val_text(c.get(1)).unwrap_or_default(),
            &text,
            &terms,
        ));
        h.mtime = mtime;
        h.agent = Some(agent);
        h.project = project;
        h.session_id = Some(sid);
        h.cwd = val_text(c.get(8)).filter(|s| !s.is_empty());
        h.score = val_f64(c.get(2)).map(rank_score);
        hits.push(h);
        if hits.len() >= limit + offset {
            break;
        }
    }
    hits.into_iter().skip(offset).take(limit).collect()
}

// ---------- 内容组 ----------

fn browse_content_group(
    conn: &Connection,
    filters: &Filters,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    let (where_sql, mut params) = sql_metadata_filters("f", filters, true);
    params.push((limit as i64).into());
    params.push((offset as i64).into());
    let sql = format!(
        "SELECT f.path, f.name, f.mtime, c.text
         FROM content c JOIN files f ON f.id=c.file_id
         WHERE 1=1{where_sql} ORDER BY f.mtime DESC LIMIT ? OFFSET ?"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        let text = r.get::<_, String>(3)?;
        let mut hit = Hit::new("content");
        hit.path = r.get(0)?;
        hit.title = r.get(1)?;
        hit.mtime = util::iso_ts(r.get(2)?);
        hit.snippet = Some(util::render_snippet(&util::make_snippet(&text, &[], 60)));
        Ok(hit)
    });
    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn search_content_group(
    conn: &Connection,
    query: &dsl::Query,
    filters: &Filters,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    if query.expr.is_none() {
        return browse_content_group(conn, filters, limit, offset);
    }
    let Some(expr) = &query.expr else {
        return Vec::new();
    };
    let terms = dsl::positive_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let open = SNIP_OPEN.to_string();
    let close = SNIP_CLOSE.to_string();
    let cap = (limit + offset) * 3 + 20;

    let cols = if let Some(fts_q) = dsl::fts5_query(query) {
        let sql = "SELECT f.path, f.name, f.mtime,
                          snippet(content_fts, 0, ?, ?, '…', 20), content_fts.rank, c.text
                   FROM content_fts
                   JOIN content c ON c.file_id = content_fts.rowid
                   JOIN files f ON f.id = content_fts.rowid
                   WHERE content_fts MATCH ?
                   ORDER BY rank LIMIT ?";
        fetch_rows(
            conn,
            sql,
            vec![open.into(), close.into(), fts_q.into(), (cap as i64).into()],
            5,
            expr,
            dsl::expr_needs_verify(expr),
            cap,
        )
    } else {
        let (likes, mut p, like_verify) = like_where("c.text", &terms, expr);
        p.push((cap as i64).into());
        let sql = format!(
            "SELECT f.path, f.name, f.mtime, '', 0.0, c.text
             FROM content c JOIN files f ON f.id = c.file_id
             WHERE ({likes}) LIMIT ?"
        );
        fetch_rows(conn, &sql, p, 5, expr, like_verify, cap)
    };

    let mut hits = Vec::new();
    for c in cols {
        let path = val_text(c.get(0));
        if !match_ext(path.as_deref(), &filters.exts) {
            continue;
        }
        if !in_prefix(path.as_deref(), &filters.prefixes) {
            continue;
        }
        let mtime = util::iso_ts(val_f64(c.get(2)));
        if !in_date(mtime.as_deref(), filters.date.as_ref()) {
            continue;
        }
        let text = val_text(c.get(5)).unwrap_or_default();
        let mut h = Hit::new("content");
        h.title = val_text(c.get(1));
        h.path = path;
        h.snippet = Some(snippet_of(
            &val_text(c.get(3)).unwrap_or_default(),
            &text,
            &terms,
        ));
        h.mtime = mtime;
        h.score = val_f64(c.get(4)).map(rank_score);
        hits.push(h);
        if hits.len() >= limit + offset {
            break;
        }
    }
    hits.into_iter().skip(offset).take(limit).collect()
}

// ---------- skill / memory 组 ----------

fn browse_agent_files_group(
    conn: &Connection,
    filters: &Filters,
    kind: &str,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    let (agent_sql, mut params) = sql_in_eq("a.agent", &filters.agents);
    let (proj_sql, proj_p) = sql_project_like("a.project", &filters.projects);
    params.extend(proj_p);
    let (meta_sql, meta_p) = sql_metadata_filters("a", filters, true);
    params.extend(meta_p);
    let extra_sql = format!("{agent_sql}{proj_sql}{meta_sql}");
    let mut all_params = Vec::new();
    all_params.push(kind.to_string().into());
    all_params.extend(params);
    all_params.push((limit as i64).into());
    all_params.push((offset as i64).into());
    let sql = format!(
        "SELECT a.agent, a.path, a.name, a.project, a.mtime, a.text
         FROM agent_files a WHERE a.kind=?{extra_sql}
         ORDER BY a.mtime DESC LIMIT ? OFFSET ?"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params_from_iter(all_params.iter()), |r| {
        let text = r.get::<_, String>(5)?;
        let mut hit = Hit::new(kind);
        hit.agent = r.get(0)?;
        hit.path = r.get(1)?;
        hit.title = r.get(2)?;
        hit.project = r.get(3)?;
        hit.mtime = util::iso_ts(r.get(4)?);
        hit.snippet = Some(util::render_snippet(&util::make_snippet(&text, &[], 60)));
        Ok(hit)
    });
    match rows {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn search_agent_files_group(
    conn: &Connection,
    query: &dsl::Query,
    filters: &Filters,
    kind: &str,
    limit: usize,
    offset: usize,
) -> Vec<Hit> {
    if query.expr.is_none() {
        return browse_agent_files_group(conn, filters, kind, limit, offset);
    }
    let Some(expr) = &query.expr else {
        return Vec::new();
    };
    let terms = dsl::positive_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let open = SNIP_OPEN.to_string();
    let close = SNIP_CLOSE.to_string();
    let cap = (limit + offset) * 3 + 20;
    let (agent_sql, agent_p) = sql_in_eq("a.agent", &filters.agents);
    let (proj_sql, proj_p) = sql_project_like("a.project", &filters.projects);
    let extra_sql = format!("{agent_sql}{proj_sql}");
    let mut extra_p = agent_p;
    extra_p.extend(proj_p);

    let cols = if let Some(fts_q) = dsl::fts5_query(query) {
        let sql = format!(
            "SELECT a.kind, a.agent, a.path, a.name, a.project, a.mtime,
                    snippet(agent_files_fts, 0, ?, ?, '…', 24), agent_files_fts.rank, a.text
             FROM agent_files_fts
             JOIN agent_files a ON a.id = agent_files_fts.rowid
             WHERE agent_files_fts MATCH ? AND a.kind = ?{extra_sql}
             ORDER BY rank LIMIT ?"
        );
        let mut p: Vec<rusqlite::types::Value> = vec![
            open.into(),
            close.into(),
            fts_q.into(),
            kind.to_string().into(),
        ];
        p.extend(extra_p);
        p.push((cap as i64).into());
        fetch_rows(conn, &sql, p, 8, expr, dsl::expr_needs_verify(expr), cap)
    } else {
        let (likes, mut p, like_verify) = like_where("a.text", &terms, expr);
        p.push(kind.to_string().into());
        p.extend(extra_p);
        p.push((cap as i64).into());
        let sql = format!(
            "SELECT a.kind, a.agent, a.path, a.name, a.project, a.mtime, '', 0.0, a.text
             FROM agent_files a WHERE ({likes}) AND a.kind = ?{extra_sql} LIMIT ?"
        );
        fetch_rows(conn, &sql, p, 8, expr, like_verify, cap)
    };

    let mut hits = Vec::new();
    for c in cols {
        let agent = val_text(c.get(1));
        if !filters.agent_ok(agent.as_deref()) {
            continue;
        }
        let project = val_text(c.get(4));
        if !filters.project_ok(project.as_deref()) {
            continue;
        }
        let path = val_text(c.get(2));
        if !match_ext(path.as_deref(), &filters.exts) {
            continue;
        }
        if !in_prefix(path.as_deref(), &filters.prefixes) {
            continue;
        }
        let mtime = util::iso_ts(val_f64(c.get(5)));
        if !in_date(mtime.as_deref(), filters.date.as_ref()) {
            continue;
        }
        let text = val_text(c.get(8)).unwrap_or_default();
        let mut h = Hit::new(kind);
        h.title = val_text(c.get(3));
        h.path = path;
        h.snippet = Some(snippet_of(
            &val_text(c.get(6)).unwrap_or_default(),
            &text,
            &terms,
        ));
        h.mtime = mtime;
        h.agent = agent;
        h.project = project;
        h.score = val_f64(c.get(7)).map(rank_score);
        hits.push(h);
        if hits.len() >= limit + offset {
            break;
        }
    }
    hits.into_iter().skip(offset).take(limit).collect()
}

// ---------- 主入口 ----------

/// 统一搜索。每组从连接池取连接 + 线程并行（LIKE 兜底场景延迟从 sum 降为 max，
/// 池化避免每查询 5 次冷开连接）。
pub fn unified_search(
    pool: &crate::db::Pool,
    names: Option<&crate::name_index::NameIndex>,
    q: &str,
    kinds: &[String],
    limit: usize,
    offset: usize,
) -> SearchResponse {
    let t0 = Instant::now();
    let query = dsl::parse(q);
    let filters = Filters::from_query(&query);
    let mut kinds: Vec<String> = kinds.to_vec();
    if let Some(kf) = &filters.kinds {
        kinds.retain(|k| kf.contains(k));
    }
    let fetch = limit + offset;
    let has_query = query.expr.is_some() || !query.filters.is_empty();

    let mut groups = serde_json::Map::new();
    let mut truncated = serde_json::Map::new();

    let results: Vec<(&str, Vec<Hit>, bool)> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for kind in ["file", "content", "session", "skill", "memory"] {
            if !kinds.iter().any(|k| k == kind) {
                continue;
            }
            let query = &query;
            let filters = &filters;
            let handle = scope.spawn(move || -> (&'static str, Vec<Hit>, bool) {
                let conn = match pool.get() {
                    Some(c) => c,
                    None => return (kind, Vec::new(), false),
                };
                let conn: &Connection = &conn;
                if !has_query {
                    return (kind, Vec::new(), false);
                }
                match kind {
                    "file" => {
                        let cands = search_file_group(&conn, names, query, filters, fetch);
                        let trunc = cands.len() >= fetch;
                        let mut page: Vec<Hit> = cands
                            .into_iter()
                            .skip(offset)
                            .take(limit)
                            .map(|c| {
                                let mut h = Hit::new("file");
                                h.title = Some(c.hit.name.clone());
                                h.path = Some(c.hit.path.clone());
                                h.is_dir = Some(c.hit.is_dir);
                                h.mtime = util::iso_ts(Some(c.hit.mtime));
                                h.score = query
                                    .expr
                                    .as_ref()
                                    .map(|_| (c.score * 10000.0).round() / 10000.0);
                                h
                            })
                            .collect();
                        // 回填：MFT 索引 mtime 恒为 0，只对返回页 stat 补 mtime；
                        // project 按会话 cwd 前缀推断
                        for h in &mut page {
                            if h.mtime.is_none() {
                                if let Some(p) = &h.path {
                                    if let Some((m, _)) =
                                        crate::message::stat_meta(std::path::Path::new(p))
                                    {
                                        h.mtime = util::iso_ts(Some(m));
                                    }
                                }
                            }
                            if h.project.is_none() {
                                if let Some(p) = h.path.clone() {
                                    h.project = project_for_path(&conn, &p);
                                }
                            }
                        }
                        (kind, page, trunc)
                    }
                    "content" => {
                        let hits = search_content_group(&conn, query, filters, limit, offset);
                        let trunc = hits.len() >= limit;
                        (kind, hits, trunc)
                    }
                    "session" => {
                        let hits = search_session_group(&conn, query, filters, limit, offset);
                        let trunc = hits.len() >= limit;
                        (kind, hits, trunc)
                    }
                    k => {
                        let hits =
                            search_agent_files_group(&conn, query, filters, k, limit, offset);
                        let trunc = hits.len() >= limit;
                        (kind, hits, trunc)
                    }
                }
            });
            handles.push(handle);
        }
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    for (kind, hits, trunc) in results {
        if trunc {
            truncated.insert(kind.into(), true.into());
        }
        groups.insert(kind.into(), serde_json::to_value(&hits).unwrap_or_default());
    }

    SearchResponse {
        q: q.to_string(),
        took_ms: t0.elapsed().as_millis() as u64,
        groups,
        truncated,
    }
}
