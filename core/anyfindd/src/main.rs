//! anyfindd：HTTP daemon，实现 docs/api-contract.md v0.1。
//! 端点：GET /api/health /api/search /api/session/{id}；POST /api/resume /api/reindex。
//! 默认端口 8765（契约），--port 可配。

use anyfind_core::name_index::NameIndex;
use anyfind_core::{agent_files, db, paths, resume, scanner, search};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

const API_VERSION: &str = "0.1.0";

type SharedNames = Arc<RwLock<NameIndex>>;

fn names_blob_path() -> PathBuf {
    paths::data_dir().join("names.bin")
}

struct Args {
    port: u16,
    root: Option<PathBuf>,
    drives: Option<Vec<String>>,
    no_index: bool,
    no_watch: bool,
    content_roots: Vec<PathBuf>,
    db: PathBuf,
}

fn parse_args() -> Args {
    let mut a = Args {
        port: 8765,
        root: None,
        drives: None,
        no_index: false,
        no_watch: false,
        content_roots: Vec::new(),
        db: paths::db_path(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => {
                if let Some(v) = it.next() {
                    a.port = v.parse().unwrap_or(8765);
                }
            }
            "--root" => {
                a.root = it.next().map(PathBuf::from);
            }
            "--drives" => {
                if let Some(v) = it.next() {
                    a.drives =
                        Some(v.split(',').map(|d| format!("{}:", d.trim_end_matches(':')).trim().to_string()).collect());
                }
            }
            "--content-root" => {
                if let Some(v) = it.next() {
                    a.content_roots.push(PathBuf::from(v));
                }
            }
            "--db" => {
                if let Some(v) = it.next() {
                    a.db = PathBuf::from(v);
                }
            }
            "--no-index" => a.no_index = true,
            "--no-watch" => a.no_watch = true,
            _ => {
                if !arg.starts_with('-') && a.root.is_none() {
                    a.root = Some(PathBuf::from(arg));
                }
            }
        }
    }
    a
}

fn open_db(db: &std::path::Path) -> Option<rusqlite::Connection> {
    let conn = db::open(db).ok()?;
    let _ = db::ensure_tables(&conn);
    Some(conn)
}

/// 统一索引（会话 + skills/记忆）后台循环：立即跑一次，之后周期性指纹增量刷新。
fn start_unified_index_loop(db_path: PathBuf, indexing: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        indexing.store(true, Ordering::Relaxed);
        if let Some(conn) = open_db(&db_path) {
            let _ = db::build_session_index(&conn);
            let _ = agent_files::build_agent_files_index(&conn);
        }
        indexing.store(false, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_secs(120));
    });
}

#[cfg(windows)]
type Watches = Vec<anyfind_core::usn::VolumeWatch>;
#[cfg(not(windows))]
type Watches = ();

/// 文件索引主体（幂等可重入，/api/reindex 复用）：MFT 全盘（需管理员）/
/// 非提权逐卷目录遍历降级 / --root 目录 / content-roots；
/// 完成后重建内存文件名索引并落盘 names.bin。
/// 返回 USN 监听上下文（仅 windows 提权全盘模式非空；no_watch=true 时不产生）。
fn file_index_body(
    db_path: &std::path::Path,
    root: &Option<PathBuf>,
    drives: &Option<Vec<String>>,
    content_roots: &[PathBuf],
    no_watch: bool,
    names: &SharedNames,
    indexing: &Arc<AtomicBool>,
) -> Watches {
    indexing.store(true, Ordering::Relaxed);
    #[cfg(windows)]
    let mut watches: Watches = Vec::new();
    #[cfg(not(windows))]
    let watches: Watches = ();
    let Some(conn) = open_db(db_path) else {
        indexing.store(false, Ordering::Relaxed);
        return watches;
    };
    #[cfg(windows)]
    {
        let elevated = anyfind_core::mft::is_elevated();
        if root.is_none() {
            if !elevated {
                // 非提权降级：逐卷目录遍历（跳系统目录）。USN 不可用，
                // 靠下方 30 分钟周期指纹重扫兜底（不捕获删除，见日志）。
                eprintln!("[anyfindd] 未提权，降级目录遍历（慢，一次性；\
                           建议管理员运行启用 MFT 秒级索引）");
                eprintln!("[anyfindd] 注意：非提权下文件名索引不实时更新，\
                           每 30 分钟指纹增量重扫兜底");
                let volumes =
                    drives.clone().unwrap_or_else(anyfind_core::mft::list_ntfs_volumes);
                for vol in &volumes {
                    let rootp = PathBuf::from(format!("{}:\\", vol.trim_end_matches(':')));
                    if !rootp.is_dir() {
                        continue;
                    }
                    let s = scanner::build_dir_index_ex(&conn, &rootp, false, true);
                    eprintln!(
                        "[anyfindd] 遍历 {vol} 完成：{} 文件 / {} 目录 / {} 跳过",
                        s.files, s.dirs, s.skipped
                    );
                    // 每卷完成即重建内存索引，让先完成的卷可搜
                    if let Ok(idx) = NameIndex::load_from_db(&conn) {
                        let _ = idx.save_blob(&names_blob_path());
                        if let Ok(mut g) = names.write() {
                            *g = idx;
                        }
                    }
                }
            } else {
                let volumes = drives.clone().unwrap_or_else(anyfind_core::mft::list_ntfs_volumes);
                scanner::bulk_begin(&conn);
                let mut degraded: Vec<String> = Vec::new();
                for vol in volumes {
                    match anyfind_core::mft::enumerate_volume(&vol) {
                        Some((entries, frn_map, journal)) => {
                            let (f, d) = scanner::insert_mft_entries(&conn, &vol, &entries);
                            eprintln!("[anyfindd] MFT {vol}: {f} 文件 / {d} 目录");
                            if !no_watch {
                                watches.push(anyfind_core::usn::VolumeWatch::from_parts(
                                    &vol, frn_map, journal,
                                ));
                            }
                        }
                        None => degraded.push(vol),
                    }
                }
                scanner::bulk_end(&conn);
                for vol in &degraded {
                    let rootp = PathBuf::from(format!("{}:\\", vol.trim_end_matches(':')));
                    if rootp.is_dir() {
                        eprintln!("[anyfindd] {vol} MFT 失败，降级目录遍历");
                        let _ = scanner::build_dir_index_ex(&conn, &rootp, false, true);
                    }
                }
            }
        }
    }
    if let Some(r) = root {
        let s = scanner::build_dir_index(&conn, r, true);
        eprintln!(
            "[anyfindd] 目录索引完成 {}：{} 文件 / {} 目录",
            r.display(),
            s.files,
            s.dirs
        );
    }
    for cr in content_roots {
        if cr.is_dir() {
            let _ = scanner::build_dir_index(&conn, cr, true);
        }
    }
    // 重建内存文件名索引 + 落盘
    if let Ok(idx) = NameIndex::load_from_db(&conn) {
        eprintln!("[anyfindd] 内存文件名索引：{} 条", idx.entries.len());
        let _ = idx.save_blob(&names_blob_path());
        if let Ok(mut g) = names.write() {
            *g = idx;
        }
    }
    indexing.store(false, Ordering::Relaxed);
    watches
}

/// 文件索引：启动时跑一次 file_index_body；USN 增量监听替代 watchdog；
/// 无 USN 时 30 分钟周期指纹重扫兜底。
fn start_file_index(args: &Args, indexing: Arc<AtomicBool>, names: SharedNames) {
    let db_path = args.db.clone();
    let root = args.root.clone();
    let drives = args.drives.clone();
    let content_roots = args.content_roots.clone();
    let no_watch = args.no_watch;

    std::thread::spawn(move || {
        let watches = file_index_body(
            &db_path, &root, &drives, &content_roots, no_watch, &names, &indexing,
        );
        // USN 增量监听（替代 watchdog）——复用枚举的 FRN 表，不二次枚举
        #[cfg(windows)]
        let mut usn_active = false;
        #[cfg(windows)]
        for watch in watches {
            start_usn_watch(&db_path, watch, names.clone());
            usn_active = true;
        }

        // 无 USN（非提权全盘 / --root 目录模式）兜底：每 30 分钟指纹增量重扫。
        // 指纹按 (mtime,size)，只补新增/修改；不捕获删除（需 MFT 模式）。
        #[cfg(windows)]
        if !usn_active {
            let db_path = db_path.clone();
            let root = root.clone();
            let drives = drives.clone();
            let names = names.clone();
            let indexing = indexing.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1800));
                if let Some(conn) = open_db(&db_path) {
                    indexing.store(true, Ordering::Relaxed);
                    match &root {
                        Some(r) => {
                            let _ = scanner::build_dir_index(&conn, r, true);
                        }
                        None => {
                            let volumes = drives
                                .clone()
                                .unwrap_or_else(anyfind_core::mft::list_ntfs_volumes);
                            for vol in &volumes {
                                let rootp =
                                    PathBuf::from(format!("{}:\\", vol.trim_end_matches(':')));
                                if rootp.is_dir() {
                                    let _ = scanner::build_dir_index_ex(&conn, &rootp, false, true);
                                }
                            }
                        }
                    }
                    if let Ok(idx) = NameIndex::load_from_db(&conn) {
                        let _ = idx.save_blob(&names_blob_path());
                        if let Ok(mut g) = names.write() {
                            *g = idx;
                        }
                    }
                    indexing.store(false, Ordering::Relaxed);
                    eprintln!("[anyfindd] 周期指纹重扫完成（30 分钟兜底）");
                }
            });
        }
    });
}

#[cfg(windows)]
fn start_usn_watch(
    db_path: &std::path::Path,
    mut watch: anyfind_core::usn::VolumeWatch,
    names: SharedNames,
) {
    let dbp = db_path.to_path_buf();
    std::thread::spawn(move || {
        let vol = watch.drive.clone();
        let stop = Arc::new(AtomicBool::new(false));
        eprintln!("[anyfindd] USN 增量监听已启动: {vol}");
        anyfind_core::usn::watch_volume(&mut watch, stop, 500, move |changes| {
            let Some(conn) = open_db(&dbp) else { return };
            for ch in changes {
                match ch {
                    anyfind_core::usn::Change::Upsert(e) => {
                        let _ = scanner::index_file(
                            &conn,
                            std::path::Path::new(&e.path),
                            false,
                        );
                        // 同步内存索引
                        if let Ok(rowid) = conn.query_row(
                            "SELECT id FROM files WHERE path=?",
                            rusqlite::params![e.path],
                            |r| r.get::<_, i64>(0),
                        ) {
                            if let Ok(mut g) = names.write() {
                                g.upsert(rowid, &e.name, e.is_dir, e.size as i64);
                            }
                        }
                    }
                    anyfind_core::usn::Change::Remove(p) => {
                        if let Ok(rowid) = conn.query_row(
                            "SELECT id FROM files WHERE path=?",
                            rusqlite::params![p],
                            |r| r.get::<_, i64>(0),
                        ) {
                            if let Ok(mut g) = names.write() {
                                g.remove(rowid);
                            }
                        }
                        scanner::remove_file(&conn, &p);
                    }
                }
            }
        });
    });
}

#[cfg(windows)]

// ---------- HTTP ----------

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            m.insert(url_decode(k), url_decode(v));
        }
    }
    m
}

fn send_json(rq: tiny_http::Request, code: u32, body: &serde_json::Value) {
    let data = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let resp = tiny_http::Response::from_string(data)
        .with_status_code(code)
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                .unwrap(),
        );
    let _ = rq.respond(resp);
}

fn health(
    db_path: &std::path::Path,
    indexing: &Arc<AtomicBool>,
    unified_indexing: &Arc<AtomicBool>,
) -> serde_json::Value {
    let (mut files, mut sessions, mut skills, mut memories) = (0i64, 0i64, 0i64, 0i64);
    if let Some(conn) = open_db(db_path) {
        files = db::file_count(&conn);
        sessions = db::session_count(&conn);
        let (s, m) = db::agent_file_counts(&conn);
        skills = s;
        memories = m;
    }
    serde_json::json!({
        "ok": true,
        "version": API_VERSION,
        "index": {"files": files, "sessions": sessions, "skills": skills, "memories": memories},
        "indexing": indexing.load(Ordering::Relaxed) || unified_indexing.load(Ordering::Relaxed),
    })
}

fn handle_get(
    rq: tiny_http::Request,
    url: &str,
    db_path: &std::path::Path,
    indexing: &Arc<AtomicBool>,
    unified_indexing: &Arc<AtomicBool>,
    names: &SharedNames,
    pool: &db::Pool,
) {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url, ""),
    };
    match path {
        "/api/health" => {
            send_json(rq, 200, &health(db_path, indexing, unified_indexing));
        }
        "/api/search" => {
            let qs = parse_query(query);
            let q = qs.get("q").cloned().unwrap_or_default();
            let kinds: Vec<String> = qs
                .get("kinds")
                .map(|k| {
                    k.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| search::ALL_KINDS.contains(&s.as_str()))
                        .collect()
                })
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| search::ALL_KINDS.iter().map(|s| s.to_string()).collect());
            let limit: usize = qs
                .get("limit")
                .and_then(|v| v.parse().ok())
                .map(|v: usize| v.clamp(1, 200))
                .unwrap_or(20);
            let offset: usize = qs.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
            let res = {
                let guard = names.read().ok();
                search::unified_search(pool, guard.as_deref(), &q, &kinds, limit, offset)
            };
            send_json(rq, 200, &serde_json::to_value(res).unwrap_or_default());
        }
        p if p.starts_with("/api/session/") => {
            let sid = url_decode(&p["/api/session/".len()..]);
            match open_db(db_path).and_then(|c| resume::get_session_messages(&c, &sid, 5000)) {
                Some(detail) => {
                    send_json(rq, 200, &serde_json::to_value(detail).unwrap_or_default())
                }
                None => send_json(
                    rq,
                    404,
                    &serde_json::json!({"error": format!("会话未找到: {sid}")}),
                ),
            }
        }
        _ => send_json(rq, 404, &serde_json::json!({"error": "not found"})),
    }
}

#[derive(Clone)]
struct IdxCfg {
    root: Option<PathBuf>,
    drives: Option<Vec<String>>,
    content_roots: Vec<PathBuf>,
}

fn handle_post(
    rq: tiny_http::Request,
    url: &str,
    db_path: &std::path::Path,
    indexing: &Arc<AtomicBool>,
    cfg: &Arc<IdxCfg>,
    names: &SharedNames,
) {
    let (path, _) = url.split_once('?').unwrap_or((url, ""));
    let mut rq = rq;
    let mut body = String::new();
    let _ = rq.as_reader().read_to_string(&mut body);
    match path {
        "/api/resume" => {
            let body: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => {
                    send_json(rq, 400, &serde_json::json!({"ok": false, "error": "请求体不是合法 JSON"}));
                    return;
                }
            };
            let agent = body.get("agent").and_then(|v| v.as_str()).unwrap_or("");
            let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            let mut cwd = body.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let resp = match open_db(db_path).and_then(|c| resume::find_session(&c, sid)) {
                None => serde_json::json!({"ok": false, "error": format!("会话未索引: {sid}")}),
                Some(meta) => {
                    if cwd.is_empty() {
                        cwd = meta.cwd.clone().unwrap_or_default();
                    }
                    let agent_eff = if agent.is_empty() { meta.agent.as_str() } else { agent };
                    match resume::resume_command(agent_eff, &meta.session_id, Some(&cwd)) {
                        Err(e) => serde_json::json!({"ok": false, "command": null, "error": e}),
                        Ok(cmd) => {
                            let mut r = serde_json::json!({"ok": true, "command": cmd});
                            if body.get("spawn").and_then(|v| v.as_bool()).unwrap_or(false) {
                                let (ok, via, serr) = resume::launch_terminal(&cmd, &cwd);
                                r["spawned"] = ok.into();
                                r["via"] = via.into();
                                if !ok {
                                    r["spawn_error"] =
                                        serde_json::Value::from(serr.unwrap_or_default());
                                }
                            }
                            r
                        }
                    }
                }
            };
            send_json(rq, 200, &resp);
        }
        "/api/reindex" => {
            // 统一索引（会话/记忆/skills）+ 文件索引重扫（--root 目录或非提权降级卷；
            // 提权 MFT 模式做一次全量重枚举，不重复挂 USN 监听）
            let indexing = indexing.clone();
            let dbp = db_path.to_path_buf();
            let cfg = cfg.clone();
            let names = names.clone();
            std::thread::spawn(move || {
                indexing.store(true, Ordering::Relaxed);
                if let Some(conn) = open_db(&dbp) {
                    let _ = db::build_session_index(&conn);
                    let _ = agent_files::build_agent_files_index(&conn);
                }
                indexing.store(false, Ordering::Relaxed);
                // 文件重扫复用启动路径（no_watch=true：监听已在跑的不重复挂）
                let file_indexing = Arc::new(AtomicBool::new(false));
                file_index_body(
                    &dbp,
                    &cfg.root,
                    &cfg.drives,
                    &cfg.content_roots,
                    true,
                    &names,
                    &file_indexing,
                );
            });
            send_json(rq, 200, &serde_json::json!({"ok": true}));
        }
        _ => send_json(rq, 404, &serde_json::json!({"error": "not found"})),
    }
}

fn main() {
    let args = parse_args();
    let indexing = Arc::new(AtomicBool::new(false));          // 文件索引（MFT/目录）
    let unified_indexing = Arc::new(AtomicBool::new(false)); // 会话/记忆索引

    // 冷启动：先装 names.bin（内存文件名索引），后台索引完成后再重建刷新
    let names: SharedNames = Arc::new(RwLock::new(NameIndex::default()));
    let blob = names_blob_path();
    if let Some(idx) = NameIndex::load_blob(&blob) {
        eprintln!("[anyfindd] names.bin 装载 {} 条: {}", idx.entries.len(), blob.display());
        if let Ok(mut g) = names.write() {
            *g = idx;
        }
    }

    if let Some(conn) = open_db(&args.db) {
        drop(conn);
        eprintln!("[anyfindd] 索引库: {}", args.db.display());
    } else {
        eprintln!("[anyfindd] 无法打开索引库: {}", args.db.display());
        std::process::exit(1);
    }
    let pool = Arc::new(db::Pool::new(&args.db, 6));
    let idx_cfg = Arc::new(IdxCfg {
        root: args.root.clone(),
        drives: args.drives.clone(),
        content_roots: args.content_roots.clone(),
    });

    if !args.no_index {
        start_unified_index_loop(args.db.clone(), unified_indexing.clone());
        start_file_index(&args, indexing.clone(), names.clone());
        // 周期性落盘 names.bin（USN 增量只改内存，重启靠快照+指纹增量收敛）
        let names_bg = names.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if let Ok(g) = names_bg.read() {
                if !g.entries.is_empty() {
                    let _ = g.save_blob(&names_blob_path());
                }
            }
        });
    }

    let addr = format!("127.0.0.1:{}", args.port);
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[anyfindd] 监听失败 {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[anyfindd] 已启动: http://{addr}");

    for rq in server.incoming_requests() {
        let indexing = indexing.clone();
        let unified_indexing = unified_indexing.clone();
        let db_path = args.db.clone();
        let names = names.clone();
        let pool = pool.clone();
        let idx_cfg = idx_cfg.clone();
        std::thread::spawn(move || {
            let url = rq.url().to_string();
            match *rq.method() {
                tiny_http::Method::Get => {
                    handle_get(rq, &url, &db_path, &indexing, &unified_indexing, &names, &pool)
                }
                tiny_http::Method::Post => {
                    handle_post(rq, &url, &db_path, &unified_indexing, &idx_cfg, &names)
                }
                _ => send_json(rq, 405, &serde_json::json!({"error": "method not allowed"})),
            }
        });
    }
}
