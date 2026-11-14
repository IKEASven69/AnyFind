//! anyfindd：HTTP daemon，实现 docs/api-contract.md v0.2。
//! 端点：GET /api/health /api/search /api/session/{id} /api/file/{list,read,stat}；
//!       POST /api/resume /api/reindex /api/file/op。
//! 默认端口 8790（契约），--port 可配。

use anyfind_core::name_index::NameIndex;
use anyfind_core::{agent_files, db, file_ops, paths, resume, scanner, search, util};
#[cfg(windows)]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

const API_VERSION: &str = "0.1.0";

type SharedNames = Arc<RwLock<NameIndex>>;

/// 健康接口只读内存计数；计数在初始化和各索引批次结束后刷新。
#[derive(Default)]
struct IndexStats {
    files: AtomicI64,
    sessions: AtomicI64,
    skills: AtomicI64,
    memories: AtomicI64,
}

impl IndexStats {
    fn refresh(&self, conn: &rusqlite::Connection) {
        let (skills, memories) = db::agent_file_counts(conn);
        self.files.store(db::file_count(conn), Ordering::Relaxed);
        self.sessions
            .store(db::session_count(conn), Ordering::Relaxed);
        self.skills.store(skills, Ordering::Relaxed);
        self.memories.store(memories, Ordering::Relaxed);
    }
}

/// 同一索引库只允许一个 anyfindd；操作系统在进程退出时自动释放锁。
struct DbInstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    lock_path: PathBuf,
}

impl DbInstanceGuard {
    fn acquire(db_path: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
            use windows_sys::Win32::System::Threading::CreateMutexW;

            let normalized = db_path.to_string_lossy().to_lowercase().replace('/', "\\");
            let mut hasher = DefaultHasher::new();
            normalized.hash(&mut hasher);
            let name = format!("Local\\AnyFindDb-{:016x}", hasher.finish());
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err("无法创建索引实例锁".into());
            }
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                return Err(format!(
                    "索引库已由另一个 AnyFind 内核使用: {}",
                    db_path.display()
                ));
            }
            Ok(Self { handle })
        }
        #[cfg(not(windows))]
        {
            let lock_path = db_path.with_extension("lock");
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
                .map_err(|_| format!("索引库已由另一个 AnyFind 内核使用: {}", db_path.display()))?;
            Ok(Self { lock_path })
        }
    }
}

impl Drop for DbInstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

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
        port: 8790,
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
                    a.port = v.parse().unwrap_or(8790);
                }
            }
            "--root" => {
                a.root = it.next().map(PathBuf::from);
            }
            "--drives" => {
                if let Some(v) = it.next() {
                    a.drives = Some(
                        v.split(',')
                            .map(|d| format!("{}:", d.trim_end_matches(':')).trim().to_string())
                            .collect(),
                    );
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
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let conn = db::open(db).ok()?;
    let _ = db::ensure_tables(&conn);
    Some(conn)
}

/// 统一索引（会话 + skills/记忆）后台循环：立即跑一次，之后周期性指纹增量刷新。
fn start_unified_index_loop(db_path: PathBuf, indexing: Arc<AtomicBool>, stats: Arc<IndexStats>) {
    std::thread::spawn(move || loop {
        indexing.store(true, Ordering::Relaxed);
        if let Some(conn) = open_db(&db_path) {
            let _ = db::build_session_index(&conn);
            let _ = agent_files::build_agent_files_index(&conn);
            stats.refresh(&conn);
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
    db_path: &Path,
    root: &Option<PathBuf>,
    drives: &Option<Vec<String>>,
    content_roots: &[PathBuf],
    no_watch: bool,
    names: &SharedNames,
    indexing: &Arc<AtomicBool>,
    stats: &Arc<IndexStats>,
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
                eprintln!(
                    "[anyfindd] 未提权，降级目录遍历（慢，一次性；\
                           建议管理员运行启用 MFT 秒级索引）"
                );
                eprintln!(
                    "[anyfindd] 注意：非提权下文件名索引不实时更新，\
                           每 30 分钟指纹增量重扫兜底"
                );
                let volumes = drives
                    .clone()
                    .unwrap_or_else(anyfind_core::mft::list_ntfs_volumes);
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
                let volumes = drives
                    .clone()
                    .unwrap_or_else(anyfind_core::mft::list_ntfs_volumes);
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
    stats.refresh(&conn);
    indexing.store(false, Ordering::Relaxed);
    watches
}

/// 文件索引：启动时跑一次 file_index_body；USN 增量监听替代 watchdog；
/// 无 USN 时 30 分钟周期指纹重扫兜底。
fn start_file_index(
    args: &Args,
    indexing: Arc<AtomicBool>,
    names: SharedNames,
    stats: Arc<IndexStats>,
) {
    let db_path = args.db.clone();
    let root = args.root.clone();
    let drives = args.drives.clone();
    let content_roots = args.content_roots.clone();
    let no_watch = args.no_watch;

    std::thread::spawn(move || {
        let watches = file_index_body(
            &db_path,
            &root,
            &drives,
            &content_roots,
            no_watch,
            &names,
            &indexing,
            &stats,
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
                        let _ = scanner::index_file(&conn, std::path::Path::new(&e.path), false);
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

/// 取请求头（大小写不敏感），值 trim 后返回。
fn header_value(rq: &tiny_http::Request, name: &'static str) -> Option<String> {
    rq.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().trim().to_string())
}

/// Host 值去端口（`127.0.0.1:8790` / `[::1]:8790` → 主机名；裸 IPv6 多冒号整体保留）。
fn host_name_of(h: &str) -> &str {
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split_once(']').map(|(v, _)| v).unwrap_or(rest);
    }
    match h.split_once(':') {
        Some((v, p)) if !v.contains(':') && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => v,
        _ => h,
    }
}

/// loopback Host 校验（DNS rebinding 防护）：内核只监听 127.0.0.1，
/// 但浏览器带恶意域名访问时 Host 是攻击者域名。Host 缺失（HTTP/1.0）
/// 放行；存在时主机名必须精确等于 loopback（大小写不敏感）。
/// 恶意页面无法伪造 Host（浏览器禁止脚本设置 Host 头），rebinding 域名
/// 会被这里拦下，无法再借 `/api/file/read` 等读本地文件。
fn loopback_host_ok(host: Option<&str>) -> bool {
    let Some(raw) = host else {
        return true;
    };
    if raw.is_empty() {
        return false;
    }
    matches!(
        host_name_of(raw).to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

/// POST /api/file/op 仅接受 `application/json`（可带 `; charset=…` 参数）。
/// 浏览器跨站表单/fetch 只能发 text/plain 等简单请求免预检，逐字符
/// JSON payload 借 text/plain 伪装发送——非 JSON Content-Type 直接拒绝。
fn json_content_type_ok(ct: Option<&str>) -> bool {
    let Some(v) = ct.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    v.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/json")
}

/// POST /api/file/op 的 Origin 门禁：内核无网页前端，正常客户端
/// （桌面壳 ureq / dsh 插件 Node 代理 / 脚本）都不携带 Origin；
/// 携带即来自浏览器跨站页面（CSRF，含 `file://` 与沙箱 iframe 的
/// `null` Origin）→ 一律拒绝。
fn origin_allowed(origin: Option<&str>) -> bool {
    origin.map(str::trim).unwrap_or("").is_empty()
}

fn send_json(rq: tiny_http::Request, code: u32, body: &serde_json::Value) {
    let data = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let resp = tiny_http::Response::from_string(data)
        .with_status_code(code)
        .with_header(
            tiny_http::Header::from_bytes(
                &b"Content-Type"[..],
                &b"application/json; charset=utf-8"[..],
            )
            .unwrap(),
        );
    let _ = rq.respond(resp);
}

fn health(
    db_path: &Path,
    stats: &IndexStats,
    indexing: &Arc<AtomicBool>,
    unified_indexing: &Arc<AtomicBool>,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "version": API_VERSION,
        "db_path": db_path.display().to_string(),
        "index": {
            "files": stats.files.load(Ordering::Relaxed),
            "sessions": stats.sessions.load(Ordering::Relaxed),
            "skills": stats.skills.load(Ordering::Relaxed),
            "memories": stats.memories.load(Ordering::Relaxed),
        },
        "indexing": indexing.load(Ordering::Relaxed) || unified_indexing.load(Ordering::Relaxed),
    })
}

// ---------- 文件端点（契约 v0.2） ----------

/// 解析 `path` 参数：缺失/非绝对 → (400, err)；不存在/不可读/链接超深 → (404, err)。
/// 返回 (原始 path 回显, canonicalize 后的真实路径)。
fn resolve_path_param(
    qs: &std::collections::HashMap<String, String>,
) -> Result<(String, PathBuf), (u32, String)> {
    let raw = qs.get("path").cloned().unwrap_or_default();
    if raw.trim().is_empty() {
        return Err((400, "缺少 path 参数".into()));
    }
    let p = file_ops::require_absolute(&raw).map_err(|e| (400, e))?;
    let resolved = file_ops::canonicalize_limited(&p).map_err(|e| (404, e))?;
    Ok((raw, resolved))
}

/// 文件 mtime → ISO（本地时区）；取不到返回 None（序列化为 null）。
fn entry_mtime(md: &std::fs::Metadata) -> Option<String> {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .and_then(|secs| util::iso_ts(Some(secs)))
}

/// canonicalize 返回的 `\\?\` / `\\?\UNC\` 前缀剥掉，交回普通路径给 PowerShell。
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest.to_string())
    } else {
        p.to_path_buf()
    }
}

/// GET /api/file/list：噪声过滤（默认隐藏）+ 目录优先排序 + max 截断。
fn file_list(rq: tiny_http::Request, qs: &std::collections::HashMap<String, String>) {
    let (raw, resolved) = match resolve_path_param(qs) {
        Ok(v) => v,
        Err((code, msg)) => {
            send_json(rq, code, &serde_json::json!({"error": msg}));
            return;
        }
    };
    let show_noise = qs.get("noise").map(|v| v == "1").unwrap_or(false);
    let max = file_ops::clamp_list_max(qs.get("max").map(|s| s.as_str()));
    let rd = match std::fs::read_dir(&resolved) {
        Ok(rd) => rd,
        Err(e) => {
            send_json(
                rq,
                404,
                &serde_json::json!({"error": format!("无法读取目录 {raw}: {e}")}),
            );
            return;
        }
    };
    let mut items = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        // 先按真实目标取元数据（链接跟到目标）；坏链退回链接自身元数据。
        let md = std::fs::metadata(ent.path()).or_else(|_| ent.metadata());
        let Ok(md) = md else { continue };
        items.push(file_ops::Entry {
            name,
            is_dir: md.is_dir(),
            size: md.len(),
            mtime: entry_mtime(&md),
        });
    }
    let (entries, truncated) = file_ops::prepare_entries(items, show_noise, max);
    let entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "is_dir": e.is_dir,
                "size": e.size,
                "mtime": e.mtime,
            })
        })
        .collect();
    send_json(
        rq,
        200,
        &serde_json::json!({"path": raw, "entries": entries, "truncated": truncated}),
    );
}

/// format=base64 通路的响应构造：(HTTP 状态, body)。
/// `file_len` = stat 尺寸；`bytes` = 整文件字节（竞态增长时以两者较大者复核 2MB 上限）。
/// >2MB → 413；否则 200 `{content_base64, binary, truncated:false}`。
fn b64_read_response(file_len: u64, bytes: &[u8]) -> (u32, serde_json::Value) {
    if file_len > file_ops::READ_SIZE_MAX || bytes.len() as u64 > file_ops::READ_SIZE_MAX {
        return (
            413,
            serde_json::json!({"error": format!(
                "文件超过 2MB（{} 字节），无法 base64 预览",
                file_len.max(bytes.len() as u64)
            )}),
        );
    }
    (
        200,
        serde_json::json!({
            "content_base64": file_ops::b64_encode(bytes),
            "binary": file_ops::is_binary_prefix(bytes),
            "truncated": false,
        }),
    )
}

/// GET /api/file/read：行分页 + 前 8KB NUL 二进制嗅探 + >2MB → 413。
fn file_read(rq: tiny_http::Request, qs: &std::collections::HashMap<String, String>) {
    let (raw, resolved) = match resolve_path_param(qs) {
        Ok(v) => v,
        Err((code, msg)) => {
            send_json(rq, code, &serde_json::json!({"error": msg}));
            return;
        }
    };
    let offset = file_ops::parse_offset(qs.get("offset").map(|s| s.as_str()));
    let lines = file_ops::clamp_read_lines(qs.get("lines").map(|s| s.as_str()));
    let md = match std::fs::metadata(&resolved) {
        Ok(md) => md,
        Err(e) => {
            send_json(
                rq,
                404,
                &serde_json::json!({"error": format!("无法读取 {raw}: {e}")}),
            );
            return;
        }
    };
    if md.is_dir() {
        send_json(
            rq,
            404,
            &serde_json::json!({"error": format!("无法按文本读取目录: {raw}")}),
        );
        return;
    }
    // format=base64：≤2MB 整文件 base64（文本与二进制通用），供 PDF/图片预览取字节；
    // 与文本通路互不影响——不传 format 走原行为。
    if qs.get("format").map(|s| s.as_str()) == Some("base64") {
        if md.len() > file_ops::READ_SIZE_MAX {
            // >2MB 仍 413（base64 通路不做二进制嗅探回退）。
            let (code, body) = b64_read_response(md.len(), &[]);
            send_json(rq, code, &body);
            return;
        }
        match std::fs::read(&resolved) {
            Ok(bytes) => {
                let (code, body) = b64_read_response(md.len(), &bytes);
                send_json(rq, code, &body);
            }
            Err(e) => {
                send_json(
                    rq,
                    404,
                    &serde_json::json!({"error": format!("无法读取 {raw}: {e}")}),
                );
            }
        }
        return;
    }
    // 嗅探先于 413（契约顺序：前 8KB 含 NUL → binary；>2MB 且非二进制 → 413）。
    if md.len() > file_ops::READ_SIZE_MAX {
        let mut probe = Vec::new();
        let probe_res = std::fs::File::open(&resolved)
            .map_err(|e| e.to_string())
            .and_then(|mut f| {
                use std::io::Read;
                (&mut f)
                    .take(file_ops::READ_SNIFF_BYTES as u64)
                    .read_to_end(&mut probe)
                    .map_err(|e| e.to_string())
            });
        match probe_res {
            Ok(_) if file_ops::is_binary_prefix(&probe) => {
                send_json(rq, 200, &serde_json::json!({"binary": true}));
            }
            Ok(_) => {
                send_json(
                    rq,
                    413,
                    &serde_json::json!({"error": format!("文件超过 2MB（{} 字节），无法预览", md.len())}),
                );
            }
            Err(e) => {
                send_json(
                    rq,
                    404,
                    &serde_json::json!({"error": format!("无法读取 {raw}: {e}")}),
                );
            }
        }
        return;
    }
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => {
            send_json(
                rq,
                404,
                &serde_json::json!({"error": format!("无法读取 {raw}: {e}")}),
            );
            return;
        }
    };
    // 嗅探先于尺寸兜底（与大文件分支同序）。
    if file_ops::is_binary_prefix(&bytes) {
        send_json(rq, 200, &serde_json::json!({"binary": true}));
        return;
    }
    // stat 与 read 之间长大的文件兜底再判一次尺寸。
    if bytes.len() as u64 > file_ops::READ_SIZE_MAX {
        send_json(
            rq,
            413,
            &serde_json::json!({"error": "文件超过 2MB，无法预览"}),
        );
        return;
    }
    let content = String::from_utf8_lossy(&bytes);
    let page = file_ops::paginate(&content, offset, lines);
    send_json(
        rq,
        200,
        &serde_json::json!({
            "content": page.lines.join("\n"),
            "totalLines": page.total,
            "offset": offset,
            "lines": page.lines.len(),
            "truncated": page.truncated,
            "binary": false,
        }),
    );
}

/// GET /api/file/stat：单路径元信息。
fn file_stat(rq: tiny_http::Request, qs: &std::collections::HashMap<String, String>) {
    let (raw, resolved) = match resolve_path_param(qs) {
        Ok(v) => v,
        Err((code, msg)) => {
            send_json(rq, code, &serde_json::json!({"error": msg}));
            return;
        }
    };
    let md = match std::fs::metadata(&resolved) {
        Ok(md) => md,
        Err(e) => {
            send_json(
                rq,
                404,
                &serde_json::json!({"error": format!("无法读取 {raw}: {e}")}),
            );
            return;
        }
    };
    let name = resolved
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| raw.clone());
    send_json(
        rq,
        200,
        &serde_json::json!({
            "name": name,
            "is_dir": md.is_dir(),
            "size": md.len(),
            "mtime": entry_mtime(&md),
        }),
    );
}

/// POST /api/file/op：结构校验（纯）→ 目录基准 canonicalize + containment 重验
/// → 根目录回收拒绝 → Windows 下交给 PowerShell 执行。
fn file_op(rq: tiny_http::Request, body: &str) {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            send_json(
                rq,
                400,
                &serde_json::json!({"error": "请求体不是合法 JSON"}),
            );
            return;
        }
    };
    let op = v
        .get("op")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let paths: Vec<String> = match v.get("paths") {
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect(),
        _ => Vec::new(),
    };
    let target = v
        .get("target")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let new_name = v
        .get("newName")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    // 1) 结构校验（纯）：op 集合 / paths ≤128 且绝对 / target、newName 保留与存在性。
    let mut items = match file_ops::plan_op(&op, &paths, target.as_deref(), new_name.as_deref()) {
        Ok(items) => items,
        Err(e) => {
            send_json(rq, 400, &serde_json::json!({"error": e}));
            return;
        }
    };
    // 2) 目录基准（rename/copy/cut 的 target；newFile/newDir 的父目录）canonicalize 后重算 dest。
    let base_raw: Option<String> = match op.as_str() {
        "rename" | "copy" | "cut" => target.clone(),
        "newFile" | "newDir" => paths.first().cloned(),
        _ => None,
    };
    if let Some(b) = base_raw {
        let resolved = match file_ops::canonicalize_limited(Path::new(&b)) {
            Ok(p) => strip_verbatim(&p),
            Err(e) => {
                send_json(
                    rq,
                    404,
                    &serde_json::json!({"error": format!("目标目录不可读: {e}")}),
                );
                return;
            }
        };
        let resolved_str = resolved.to_string_lossy().into_owned();
        let (paths2, target2): (Vec<String>, Option<String>) =
            if matches!(op.as_str(), "newFile" | "newDir") {
                let mut p2 = paths.clone();
                p2[0] = resolved_str;
                (p2, None)
            } else {
                (paths.clone(), Some(resolved_str))
            };
        match file_ops::plan_op(&op, &paths2, target2.as_deref(), new_name.as_deref()) {
            Ok(mut items2) => {
                // results 回显请求原路径，不回显 canonicalize 后的路径。
                for (i, it) in items2.iter_mut().enumerate() {
                    if let Some(p) = paths.get(i) {
                        it.path = p.clone();
                    }
                }
                items = items2;
            }
            Err(e) => {
                send_json(rq, 400, &serde_json::json!({"error": e}));
                return;
            }
        }
    }
    // 3) recycle：拒绝盘符/UNC 根（canonicalize 后判断，`D:\..` 也算根）。
    if op == "recycle" {
        for it in &items {
            if let Ok(c) = file_ops::canonicalize_limited(Path::new(&it.src)) {
                if file_ops::is_root_path(&c) {
                    send_json(
                        rq,
                        400,
                        &serde_json::json!({"error": format!("拒绝操作根目录: {}", it.path)}),
                    );
                    return;
                }
            }
        }
    }
    // 4) 平台门。
    #[cfg(not(windows))]
    {
        let _ = items;
        send_json(
            rq,
            501,
            &serde_json::json!({"error": "文件操作仅支持 Windows 平台"}),
        );
    }
    #[cfg(windows)]
    {
        match run_file_op(op.as_str(), &items) {
            Ok(results) => send_json(
                rq,
                200,
                &serde_json::json!({"ok": true, "results": results}),
            ),
            Err(e) => send_json(rq, 500, &serde_json::json!({"error": e})),
        }
    }
}

/// base64 标准字母表编码（stdin payload 与 -EncodedCommand 共用）；
/// 实现统一到 file_ops::b64_encode（跨平台纯函数，含已知向量测试）。
#[cfg(windows)]
fn b64encode(data: &[u8]) -> String {
    file_ops::b64_encode(data)
}

/// powershell 单批操作限时（25s）：小于桌面壳 30s / dsh 插件 60s 的请求超时，
/// 保证内核先于客户端回错误，而不是让调用方把超时误判成"内核未启动"。
#[cfg(windows)]
const PS_OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

/// 六类文件操作的 PowerShell 执行脚本（payload 经 stdin 以 base64 单行传入，
/// 输出单行 JSON `{results:[{path,ok,error?}]}`）。
/// recycle 用 SHFileOperation 静默入回收站（FOF_NOERRORUI/SILENT/NOCONFIRMATION）：
/// 被占用等失败直接进逐项 error（契约 200 + ok:false），不弹 Win32 模态对话框。
#[cfg(windows)]
const PS_OP_SCRIPT: &str = r##"$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$b64 = [Console]::In.ReadLine()
$json = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($b64))
$req = $json | ConvertFrom-Json
$op = [string]$req.op
$items = @($req.items)
if ($op -eq 'recycle') {
  # 静默回收用 SHFileOperation：FOF_ALLOWUNDO（入回收站）+ FOF_SILENT / FOF_NOCONFIRMATION /
  # FOF_NOERRORUI（无任何 UI）。文件被占用、无权限等失败只返回非零错误码，绝不弹模态
  # 对话框把子进程挂住（VB FileIO 的 UIOption 只有 OnlyErrorDialogs/AllDialogs，被占用必弹框，故弃用）。
  Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class AnyFindRecycle
{
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct SHFILEOPSTRUCT
  {
    public IntPtr hwnd;
    [MarshalAs(UnmanagedType.U4)] public uint wFunc;
    public string pFrom;
    public string pTo;
    public ushort fFlags;
    [MarshalAs(UnmanagedType.Bool)] public bool fAnyOperationsAborted;
    public IntPtr hNameMappings;
    public string lpszProgressTitle;
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
  public static extern int SHFileOperation(ref SHFILEOPSTRUCT lpFileOp);
  public static int Recycle(string path)
  {
    SHFILEOPSTRUCT op = new SHFILEOPSTRUCT();
    op.hwnd = IntPtr.Zero;
    op.wFunc = 3;             // FO_DELETE
    op.pFrom = path + "\0";   // pFrom 需双 NUL 结尾
    op.pTo = null;
    op.fFlags = (ushort)(0x40 | 0x10 | 0x4 | 0x400); // FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI
    return SHFileOperation(ref op);
  }
}
"@
}
$results = @()
foreach ($it in $items) {
  $p = [string]$it.path; $src = [string]$it.src; $dest = [string]$it.dest
  $ok = $false; $err = $null
  try {
    switch ($op) {
      'newFile' { if (Test-Path -LiteralPath $dest) { throw "目标已存在: $dest" }; $par = [System.IO.Path]::GetDirectoryName($dest); if ($par -and -not [System.IO.Directory]::Exists($par)) { throw "父目录不存在: $par" }; [System.IO.File]::Create($dest).Dispose() }
      'copy' { if (-not (Test-Path -LiteralPath $src)) { throw "源不存在: $src" }; if (Test-Path -LiteralPath $dest) { throw "目标已存在: $dest" }; Copy-Item -LiteralPath $src -Destination $dest -Recurse -Force -ErrorAction Stop }
      'rename' { if (-not (Test-Path -LiteralPath $src)) { throw "源不存在: $src" }; if (Test-Path -LiteralPath $dest) { throw "目标已存在: $dest" }; $gi = Get-Item -LiteralPath $src -Force; if ($gi.PSIsContainer) { [System.IO.Directory]::Move($src, $dest) } else { [System.IO.File]::Move($src, $dest) } }
      'cut' { if (-not (Test-Path -LiteralPath $src)) { throw "源不存在: $src" }; if (Test-Path -LiteralPath $dest) { throw "目标已存在: $dest" }; Move-Item -LiteralPath $src -Destination $dest -ErrorAction Stop }
      'recycle' { if (-not (Test-Path -LiteralPath $src)) { throw "源不存在: $src" }; $ret = [AnyFindRecycle]::Recycle($src); if ($ret -ne 0) { throw ("回收失败（SHFileOperation 代码 0x{0:X2}，常见原因：文件被占用/拒绝访问/源不可用）" -f $ret) } }
      'newDir' { if (Test-Path -LiteralPath $dest) { throw "目标已存在: $dest" }; $par = [System.IO.Path]::GetDirectoryName($dest); if ($par -and -not [System.IO.Directory]::Exists($par)) { throw "父目录不存在: $par" }; [System.IO.Directory]::CreateDirectory($dest) | Out-Null }
      default { throw "未知 op: $op" }
    }
    $ok = $true
  } catch { $err = $_.Exception.Message; if (-not $err) { $err = [string]$_ } }
  if ($ok) { $results += [pscustomobject]@{ path = $p; ok = $true } }
  else { $results += [pscustomobject]@{ path = $p; ok = $false; error = $err } }
}
[Console]::Out.WriteLine((ConvertTo-Json -InputObject ([pscustomobject]@{ results = $results }) -Compress -Depth 5))"##;

/// 拉起 powershell 执行一批操作，返回与请求等长的 results 数组。
#[cfg(windows)]
fn run_file_op(op: &str, items: &[file_ops::OpItem]) -> Result<Vec<serde_json::Value>, String> {
    let payload = serde_json::json!({
        "op": op,
        "items": items
            .iter()
            .map(|i| serde_json::json!({"path": i.path, "src": i.src, "dest": i.dest}))
            .collect::<Vec<_>>(),
    });
    let payload_b64 = b64encode(
        serde_json::to_string(&payload)
            .map_err(|e| format!("序列化操作 payload 失败: {e}"))?
            .as_bytes(),
    );
    let script_utf16: Vec<u8> = PS_OP_SCRIPT
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let enc = b64encode(&script_utf16);
    // 注意：-ExecutionPolicy Bypass 置于 -EncodedCommand 前在部分策略环境下会挂起
    // （本机实测），且 EncodedCommand 脚本不受执行策略限制——故不加该参数。
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &enc])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 powershell 失败: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "无法写入 powershell stdin".to_string())?;
        stdin
            .write_all(payload_b64.as_bytes())
            .map_err(|e| format!("写入 payload 失败: {e}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| format!("写入 payload 失败: {e}"))?;
        // stdin 在此作用域结束时关闭，脚本 ReadLine 拿到完整行。
    }
    // 限时等待：stdout/stderr 由两个线程并发排空（防管道满阻塞子进程），
    // 主线程轮询 try_wait；超时（如回收被系统占用文件导致子进程卡住）杀掉
    // powershell 并返回错误，HTTP 请求线程不再无限挂起。
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取 powershell stdout".to_string())?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 powershell stderr".to_string())?;
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout_pipe, &mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr_pipe, &mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + PS_OP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("等待 powershell 失败: {e}"));
            }
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(format!(
            "powershell 执行超时（{}s），已终止",
            PS_OP_TIMEOUT.as_secs()
        ));
    };
    let out = std::process::Output {
        status,
        stdout,
        stderr,
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!(
            "powershell 退出 {:?}: {} | stdout: {}",
            out.status.code(),
            stderr.chars().take(300).collect::<String>(),
            stdout.chars().take(300).collect::<String>(),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim_start_matches('\u{feff}').trim();
    let parsed: serde_json::Value = serde_json::from_str(trimmed).or_else(|e| {
        // 兜底：万一 stdout 混入宿主输出，取最后一个可解析 JSON 的行。
        trimmed
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str(l.trim()).ok())
            .ok_or_else(|| format!("解析 powershell 输出失败: {e}; stdout: {trimmed}"))
    })?;
    let results = parsed
        .get("results")
        .and_then(|r| r.as_array())
        .ok_or_else(|| format!("powershell 输出缺少 results: {trimmed}"))?;
    Ok(results.clone())
}

fn handle_get(
    rq: tiny_http::Request,
    url: &str,
    db_path: &Path,
    stats: &Arc<IndexStats>,
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
            send_json(rq, 200, &health(db_path, stats, indexing, unified_indexing));
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
        "/api/file/list" => {
            let qs = parse_query(query);
            file_list(rq, &qs);
        }
        "/api/file/read" => {
            let qs = parse_query(query);
            file_read(rq, &qs);
        }
        "/api/file/stat" => {
            let qs = parse_query(query);
            file_stat(rq, &qs);
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
    db_path: &Path,
    indexing: &Arc<AtomicBool>,
    stats: &Arc<IndexStats>,
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
                    send_json(
                        rq,
                        400,
                        &serde_json::json!({"ok": false, "error": "请求体不是合法 JSON"}),
                    );
                    return;
                }
            };
            let agent = body.get("agent").and_then(|v| v.as_str()).unwrap_or("");
            let sid = body
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut cwd = body
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let resp = match open_db(db_path).and_then(|c| resume::find_session(&c, sid)) {
                None => serde_json::json!({"ok": false, "error": format!("会话未索引: {sid}")}),
                Some(meta) => {
                    if cwd.is_empty() {
                        cwd = meta.cwd.clone().unwrap_or_default();
                    }
                    let agent_eff = if agent.is_empty() {
                        meta.agent.as_str()
                    } else {
                        agent
                    };
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
            let stats = stats.clone();
            let dbp = db_path.to_path_buf();
            let cfg = cfg.clone();
            let names = names.clone();
            std::thread::spawn(move || {
                indexing.store(true, Ordering::Relaxed);
                if let Some(conn) = open_db(&dbp) {
                    let _ = db::build_session_index(&conn);
                    let _ = agent_files::build_agent_files_index(&conn);
                    stats.refresh(&conn);
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
                    &stats,
                );
            });
            send_json(rq, 200, &serde_json::json!({"ok": true}));
        }
        "/api/file/op" => {
            // CSRF 门禁（文件操作是破坏性端点，浏览器跨站页面可发免预检简单请求）：
            // 1) 任何非空 Origin（含 file:// / 沙箱 iframe 的 `null`）→ 403；
            // 2) Content-Type 必须 application/json（text/plain 等伪装体 → 415）。
            let origin = header_value(&rq, "Origin");
            if !origin_allowed(origin.as_deref()) {
                eprintln!("[anyfindd] 拒绝跨站 file/op 请求: Origin={origin:?}");
                send_json(rq, 403, &serde_json::json!({"error": "拒绝跨站请求：内核仅接受本机客户端（不带 Origin）"}));
                return;
            }
            let ct = header_value(&rq, "Content-Type");
            if !json_content_type_ok(ct.as_deref()) {
                send_json(rq, 415, &serde_json::json!({
                    "error": format!("Content-Type 必须是 application/json（收到: {}）", ct.as_deref().unwrap_or("缺失")),
                }));
                return;
            }
            file_op(rq, &body);
        }
        _ => send_json(rq, 404, &serde_json::json!({"error": "not found"})),
    }
}

fn main() {
    let args = parse_args();
    // names.bin 必须跟随 --db，而不是继续写入默认配置盘。
    if let Some(dir) = args.db.parent() {
        let _ = std::fs::create_dir_all(dir);
        std::env::set_var("ANYFIND_DATA_DIR", dir);
    }
    let _instance = match DbInstanceGuard::acquire(&args.db) {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("[anyfindd] {e}");
            std::process::exit(2);
        }
    };
    let indexing = Arc::new(AtomicBool::new(false)); // 文件索引（MFT/目录）
    let unified_indexing = Arc::new(AtomicBool::new(false)); // 会话/记忆索引
    let stats = Arc::new(IndexStats::default());

    // 冷启动：先装 names.bin（内存文件名索引），后台索引完成后再重建刷新
    let names: SharedNames = Arc::new(RwLock::new(NameIndex::default()));
    let blob = names_blob_path();
    if let Some(idx) = NameIndex::load_blob(&blob) {
        eprintln!(
            "[anyfindd] names.bin 装载 {} 条: {}",
            idx.entries.len(),
            blob.display()
        );
        if let Ok(mut g) = names.write() {
            *g = idx;
        }
    }

    if let Some(conn) = open_db(&args.db) {
        stats.refresh(&conn);
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

    // 先提供轻量健康与搜索接口，再启动可能耗时较长的后台索引。
    let addr = format!("127.0.0.1:{}", args.port);
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[anyfindd] 监听失败 {addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[anyfindd] 已启动: http://{addr}");

    if !args.no_index {
        start_unified_index_loop(args.db.clone(), unified_indexing.clone(), stats.clone());
        start_file_index(&args, indexing.clone(), names.clone(), stats.clone());
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

    for rq in server.incoming_requests() {
        let indexing = indexing.clone();
        let unified_indexing = unified_indexing.clone();
        let stats = stats.clone();
        let db_path = args.db.clone();
        let names = names.clone();
        let pool = pool.clone();
        let idx_cfg = idx_cfg.clone();
        std::thread::spawn(move || {
            // CSRF/DNS-rebinding 门禁（所有端点统一）：Host 必须是 loopback。
            let host = header_value(&rq, "Host");
            if !loopback_host_ok(host.as_deref()) {
                eprintln!("[anyfindd] 拒绝非本机 Host 请求: {:?}", host);
                send_json(rq, 403, &serde_json::json!({"error": "拒绝访问：Host 非本机回环地址"}));
                return;
            }
            let url = rq.url().to_string();
            match *rq.method() {
                tiny_http::Method::Get => handle_get(
                    rq,
                    &url,
                    &db_path,
                    &stats,
                    &indexing,
                    &unified_indexing,
                    &names,
                    &pool,
                ),
                tiny_http::Method::Post => handle_post(
                    rq,
                    &url,
                    &db_path,
                    &unified_indexing,
                    &stats,
                    &idx_cfg,
                    &names,
                ),
                _ => send_json(rq, 405, &serde_json::json!({"error": "method not allowed"})),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        entry_mtime, health, host_name_of, json_content_type_ok, loopback_host_ok,
        origin_allowed, resolve_path_param, strip_verbatim, DbInstanceGuard, IndexStats,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn qs(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_path_param_maps_missing_relative_and_absent() {
        // 缺 path → 400
        let (code, _) = resolve_path_param(&qs(&[])).unwrap_err();
        assert_eq!(code, 400);
        // 空白 path → 400
        let (code, _) = resolve_path_param(&qs(&[("path", "   ")])).unwrap_err();
        assert_eq!(code, 400);
        // 相对路径 → 400
        let (code, _) = resolve_path_param(&qs(&[("path", "relative/dir")])).unwrap_err();
        assert_eq!(code, 400);
        // 绝对但不存在 → 404
        let (code, _) =
            resolve_path_param(&qs(&[("path", "Z:\\__anyfind_no_such_dir__\\x")])).unwrap_err();
        assert_eq!(code, 404);
        // 存在的临时目录 → Ok，回显原 path
        let tmp = std::env::temp_dir();
        let raw = tmp.to_string_lossy().into_owned();
        let (echo, resolved) = resolve_path_param(&qs(&[("path", raw.as_str())])).unwrap();
        assert_eq!(echo, raw);
        assert!(resolved.is_absolute());
        assert!(!file_ops_exists(&resolved));
    }

    // canonicalize 结果应为已归一路径（不残留 .. 组件）。
    fn file_ops_exists(p: &Path) -> bool {
        let s = p.to_string_lossy();
        s.contains("..") || s.contains("/./")
    }

    #[test]
    fn strip_verbatim_rewrites_unc_and_disk_prefixes() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\D:\coding\a.txt")),
            PathBuf::from(r"D:\coding\a.txt")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share\x")),
            PathBuf::from(r"\\srv\share\x")
        );
        // 普通路径原样
        assert_eq!(
            strip_verbatim(Path::new(r"D:\coding\a.txt")),
            PathBuf::from(r"D:\coding\a.txt")
        );
    }

    #[test]
    fn entry_mtime_formats_iso_or_none() {
        let f = std::env::temp_dir().join(format!(
            "anyfind-mtime-test-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&f, b"x").unwrap();
        let md = std::fs::metadata(&f).unwrap();
        let iso = entry_mtime(&md).expect("mtime 应可格式化");
        // 与桌面 UI mock 同格式：YYYY-MM-DDTHH:MM:SS
        assert_eq!(iso.len(), 19);
        assert_eq!(iso.as_bytes()[4], b'-');
        assert_eq!(iso.as_bytes()[10], b'T');
        let _ = std::fs::remove_file(&f);
    }

    #[cfg(windows)]
    #[test]
    fn b64encode_matches_known_vectors() {
        use super::b64encode;
        assert_eq!(b64encode(b""), "");
        assert_eq!(b64encode(b"f"), "Zg==");
        assert_eq!(b64encode(b"fo"), "Zm8=");
        assert_eq!(b64encode(b"foo"), "Zm9v");
        assert_eq!(b64encode(b"foobar"), "Zm9vYmFy");
        // UTF-16LE 编码链：脚本经 -EncodedCommand 传入前的形态
        let enc = b64encode(
            &"A".encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect::<Vec<u8>>(),
        );
        assert_eq!(enc, "QQA=");
    }

    #[test]
    fn b64_read_response_text_binary_and_over_2mb() {
        use super::{b64_read_response, file_ops};
        // 文本 base64：无 NUL → binary:false，字节精确编码
        let (code, body) = b64_read_response(5, b"hello");
        assert_eq!(code, 200);
        assert_eq!(body["content_base64"], "aGVsbG8=");
        assert_eq!(body["binary"], false);
        assert_eq!(body["truncated"], false);
        // 二进制 base64：前 8KB 含 NUL → binary:true，字节仍完整给出（PDF/图片预览用途）
        let bin: &[u8] = b"%PDF-1.7\n\0\0tail";
        let (code, body) = b64_read_response(bin.len() as u64, bin);
        assert_eq!(code, 200);
        assert_eq!(body["binary"], true);
        assert_eq!(body["content_base64"], file_ops::b64_encode(bin));
        assert_eq!(body["truncated"], false);
        // >2MB 仍 413（stat 维度）
        let big = vec![b'x'; file_ops::READ_SIZE_MAX as usize + 1];
        let (code, body) = b64_read_response(big.len() as u64, &big);
        assert_eq!(code, 413);
        assert!(body["error"].as_str().unwrap().contains("2MB"));
        // stat ≤2MB 但实际字节超限（读取竞态）→ 仍 413
        let (code, body) = b64_read_response(10, &big);
        assert_eq!(code, 413);
    }

    #[test]
    fn health_reads_cached_counts_without_opening_db() {
        let stats = IndexStats::default();
        stats.files.store(5_424_321, Ordering::Relaxed);
        stats.sessions.store(42, Ordering::Relaxed);
        stats.skills.store(7, Ordering::Relaxed);
        stats.memories.store(9, Ordering::Relaxed);
        let value = health(
            Path::new("Z:/path-that-must-not-be-opened/index.db"),
            &stats,
            &Arc::new(Default::default()),
            &Arc::new(Default::default()),
        );
        assert_eq!(value["index"]["files"], 5_424_321);
        assert_eq!(value["index"]["sessions"], 42);
        assert_eq!(value["index"]["skills"], 7);
        assert_eq!(value["index"]["memories"], 9);
    }

    #[test]
    fn db_instance_guard_is_exclusive_and_released_on_drop() {
        let root = std::env::temp_dir().join(format!(
            "anyfind-instance-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("index.db");
        let first = DbInstanceGuard::acquire(&db).unwrap();
        assert!(DbInstanceGuard::acquire(&db).is_err());
        let other = DbInstanceGuard::acquire(&root.join("other.db")).unwrap();
        drop(other);
        drop(first);
        let reacquired = DbInstanceGuard::acquire(&db).unwrap();
        drop(reacquired);
        let _ = std::fs::remove_dir_all(PathBuf::from(root));
    }

    // ---- CSRF / DNS rebinding 门禁 ----

    #[test]
    fn loopback_host_ok_accepts_only_loopback() {
        // 缺失 Host（HTTP/1.0 客户端）放行
        assert!(loopback_host_ok(None));
        // 常见合法形态：带端口 / 不带端口 / localhost / IPv6 / 大小写
        assert!(loopback_host_ok(Some("127.0.0.1:8790")));
        assert!(loopback_host_ok(Some("127.0.0.1")));
        assert!(loopback_host_ok(Some("localhost:18990")));
        assert!(loopback_host_ok(Some("LOCALHOST")));
        assert!(loopback_host_ok(Some("[::1]:8790")));
        assert!(loopback_host_ok(Some("::1")));
        // DNS rebinding 域名与伪造 Host 全部拒绝
        assert!(!loopback_host_ok(Some("evil.example")));
        assert!(!loopback_host_ok(Some("127.0.0.1.evil.example")));
        assert!(!loopback_host_ok(Some("evil.example:80")));
        assert!(!loopback_host_ok(Some("192.168.1.2:8790")));
        assert!(!loopback_host_ok(Some("::ffff:127.0.0.1")));
        // 空 Host 视为畸形，拒绝
        assert!(!loopback_host_ok(Some("   ")));
    }

    #[test]
    fn host_name_of_strips_port_and_brackets() {
        assert_eq!(host_name_of("127.0.0.1:8790"), "127.0.0.1");
        assert_eq!(host_name_of("localhost:18990"), "localhost");
        assert_eq!(host_name_of("evil.example"), "evil.example");
        assert_eq!(host_name_of("evil.example:80"), "evil.example");
        assert_eq!(host_name_of("[::1]:8790"), "::1");
        assert_eq!(host_name_of("::1"), "::1");
    }

    #[test]
    fn file_op_content_type_guard() {
        assert!(json_content_type_ok(Some("application/json")));
        assert!(json_content_type_ok(Some("application/json; charset=utf-8")));
        assert!(json_content_type_ok(Some("APPLICATION/JSON")));
        assert!(json_content_type_ok(Some("application/json;charset=UTF-8")));
        // 浏览器跨站简单请求的伪装 Content-Type 一律拒绝
        assert!(!json_content_type_ok(Some("text/plain")));
        assert!(!json_content_type_ok(Some("text/plain; charset=utf-8")));
        assert!(!json_content_type_ok(Some("application/x-www-form-urlencoded")));
        assert!(!json_content_type_ok(Some("multipart/form-data")));
        assert!(!json_content_type_ok(Some("text/json")));
        // 缺失 / 空 → 拒绝（浏览器总是发送 Content-Type，缺失即非浏览器常规客户端异常请求）
        assert!(!json_content_type_ok(None));
        assert!(!json_content_type_ok(Some("")));
    }

    #[test]
    fn file_op_origin_guard_rejects_any_origin() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("")));
        assert!(origin_allowed(Some("   ")));
        // 跨站页面（http/https）与 file://、沙箱 iframe 的 null Origin 均拒绝
        assert!(!origin_allowed(Some("http://evil.example")));
        assert!(!origin_allowed(Some("https://evil.example:8080")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("http://localhost:5173")));
    }
}
