//! 目录遍历文件索引（MFT 不可用时的降级路径）+ 内容索引。
//! 移植自 filefind/scanner.py。

use crate::message::stat_meta;
use crate::util;
use rusqlite::{params, Connection};
use std::path::Path;
use walkdir::WalkDir;

pub const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024; // 20MB，不索引超大文件内容

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "__pycache__", ".venv", "venv",
    ".idea", ".vscode", "dist", "build", ".opencode", "target",
];

const TEXT_EXT: &[&str] = &[
    ".txt", ".md", ".py", ".js", ".ts", ".tsx", ".jsx", ".go", ".rs", ".c",
    ".h", ".cpp", ".hpp", ".java", ".kt", ".rb", ".php", ".sh", ".bat",
    ".ps1", ".json", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".xml",
    ".html", ".css", ".sql", ".log", ".csv", ".rst", ".tex", ".lua",
    ".r", ".swift", ".scala", ".pl", ".dockerfile",
];

pub fn pinyin_of(name: &str) -> (String, String) {
    // 纯 ASCII 名字的拼音就是其本身，跳过查表（MFT 全量 2M 行时占 90%+ 耗时）
    if name.is_ascii() {
        return (String::new(), String::new());
    }
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let py: String = pinyin::to_pinyin_vec(&stem, pinyin::Pinyin::plain).concat();
    let init: String = pinyin::to_pinyin_vec(&stem, pinyin::Pinyin::first_letter)
        .concat()
        .chars()
        .filter(|c| c.is_ascii_lowercase())
        .collect();
    (py.to_lowercase(), init)
}

fn is_text(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if TEXT_EXT.contains(&ext.as_str()) {
        return true;
    }
    // 未知扩展名：读头 4KB 探测 UTF-8
    use std::io::Read;
    let mut buf = [0u8; 4096];
    match std::fs::File::open(path) {
        Ok(mut f) => match f.read(&mut buf) {
            Ok(0) => true,
            Ok(n) => std::str::from_utf8(&buf[..n]).is_ok(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// 索引单个文件/目录（upsert files；文本文件顺带建内容索引）。返回是否更新。
pub fn index_file(conn: &Connection, full_path: &Path, with_content: bool) -> bool {
    let is_dir = full_path.is_dir();
    if !is_dir && !full_path.is_file() {
        return false;
    }
    let Some((mtime, size)) = stat_meta(full_path) else { return false };
    if !is_dir && size > MAX_FILE_SIZE {
        return false;
    }
    let db_path = full_path.to_string_lossy().into_owned();
    let name = full_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| db_path.clone());

    let cur: Option<(i64, f64, i64)> = conn
        .query_row(
            "SELECT id, mtime, indexed FROM files WHERE path=?",
            params![db_path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    if let Some((_, m, indexed)) = cur {
        if (m - mtime).abs() < 1e-3 && indexed == 1 && !with_content {
            return false;
        }
    }

    let fid = if let Some((fid, _, _)) = cur {
        let _ = conn.execute(
            "UPDATE files SET name=?, size=?, mtime=?, is_dir=?, indexed=0 WHERE id=?",
            params![name, size as i64, mtime, is_dir as i64, fid],
        );
        fid
    } else {
        let (py, init) = pinyin_of(&name);
        let _ = conn.execute(
            "INSERT INTO files (path, name, pinyin, pinyin_init, size, mtime, is_dir, indexed)
             VALUES (?,?,?,?,?,?,?,0)",
            params![db_path, name, py, init, size as i64, mtime, is_dir as i64],
        );
        conn.last_insert_rowid()
    };

    if is_dir {
        let _ = conn.execute("UPDATE files SET indexed=1 WHERE id=?", params![fid]);
        return true;
    }
    if with_content && is_text(full_path) {
        if let Ok(bytes) = std::fs::read(full_path) {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO content (file_id, text) VALUES (?,?)",
                params![fid, text],
            );
            let _ = conn.execute("UPDATE files SET indexed=1 WHERE id=?", params![fid]);
        }
    } else {
        let _ = conn.execute("UPDATE files SET indexed=1 WHERE id=?", params![fid]);
    }
    true
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ScanStats {
    pub files: usize,
    pub dirs: usize,
    pub indexed: usize,
    pub skipped: usize,
}

/// 卷根降级遍历时的跳过目录（仅 depth==1 生效）：系统/程序目录不索引，
/// 否则 C 盘遍历又慢又蠢（MFT 模式无此限制，全量索引）。
const VOLUME_ROOT_SKIP: &[&str] = &[
    "windows", "program files", "program files (x86)", "$recycle.bin",
    "system volume information", "recovery", "documents and settings", "perflogs",
];

/// 遍历 root 建文件名索引（with_content 时顺带内容索引）。
/// volume_root=true 时按 VOLUME_ROOT_SKIP 跳过卷根系统目录（降级全盘遍历用）。
/// 不跟随 junction/symlink（避免 C:\Documents and Settings→Users 这类重复遍历）。
pub fn build_dir_index_ex(
    conn: &Connection,
    root: &Path,
    with_content: bool,
    volume_root: bool,
) -> ScanStats {
    let mut stats = ScanStats::default();
    for e in WalkDir::new(root).into_iter().filter_entry(|e| {
        if e.depth() > 0 && e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if SKIP_DIRS.contains(&name.as_str()) {
                return false;
            }
            if volume_root && e.depth() == 1 && VOLUME_ROOT_SKIP.contains(&name.as_str()) {
                return false;
            }
        }
        true
    }) {
        let Ok(e) = e else { continue };
        let p = e.path();
        if e.file_type().is_dir() {
            stats.dirs += 1;
            if index_file(conn, p, false) {
                stats.indexed += 1;
            }
        } else if e.file_type().is_file() {
            match stat_meta(p) {
                Some((_, size)) if size > MAX_FILE_SIZE => stats.skipped += 1,
                Some(_) => {
                    stats.files += 1;
                    if index_file(conn, p, with_content) {
                        stats.indexed += 1;
                    }
                }
                None => stats.skipped += 1,
            }
        }
    }
    stats
}

/// 遍历 root 建文件名索引（with_content 时顺带内容索引）。
pub fn build_dir_index(conn: &Connection, root: &Path, with_content: bool) -> ScanStats {
    build_dir_index_ex(conn, root, with_content, false)
}

#[derive(Debug, Clone)]
pub struct NameHit {
    pub path: String,
    pub name: String,
    pub size: i64,
    pub mtime: f64,
    pub is_dir: bool,
}

/// 文件名搜索：name/path/拼音 LIKE。query 为完整子串（调用方按词拆分循环用）。
pub fn search_name(conn: &Connection, query: &str, limit: usize) -> Vec<NameHit> {
    let like = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
    let is_ascii_alpha = query.is_ascii() && query.chars().all(|c| c.is_ascii_alphabetic());
    let sql = if is_ascii_alpha {
        "SELECT path, name, size, mtime, is_dir FROM files
         WHERE name LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\'
            OR pinyin LIKE ? ESCAPE '\\' OR pinyin_init LIKE ? ESCAPE '\\'
         ORDER BY is_dir DESC,
           CASE WHEN pinyin LIKE ? ESCAPE '\\' THEN 0
                WHEN pinyin_init LIKE ? ESCAPE '\\' THEN 1 ELSE 2 END,
           size DESC LIMIT ?"
    } else {
        "SELECT path, name, size, mtime, is_dir FROM files
         WHERE name LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\'
         ORDER BY is_dir DESC, size DESC LIMIT ?"
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let map = |r: &rusqlite::Row| -> rusqlite::Result<NameHit> {
        Ok(NameHit {
            path: r.get(0)?,
            name: r.get(1)?,
            size: r.get(2)?,
            mtime: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
            is_dir: r.get::<_, i64>(4)? != 0,
        })
    };
    let rows = if is_ascii_alpha {
        stmt.query_map(params![like, like, like, like, like, like, limit as i64], map)
    } else {
        stmt.query_map(params![like, like, limit as i64], map)
    };
    match rows {
        Ok(rs) => rs.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// 单个文件移出索引（USN 增量删除用）。
pub fn remove_file(conn: &Connection, path: &str) -> bool {
    let fid: Option<i64> = conn
        .query_row("SELECT id FROM files WHERE path=?", params![path], |r| r.get(0))
        .ok();
    let Some(fid) = fid else { return false };
    let _ = conn.execute("DELETE FROM content WHERE file_id=?", params![fid]);
    let _ = conn.execute("DELETE FROM files WHERE id=?", params![fid]);
    true
}

pub fn iso_of(mtime: f64) -> Option<String> {
    util::iso_ts(Some(mtime))
}

/// 批量模式开始：关同步 + 内存 journal + 有界页缓存 + 暂缓二级索引。
/// 配 bulk_end 使用；崩溃则全量重来，可接受。
pub fn bulk_begin(conn: &Connection) {
    let _ = conn.pragma_update(None, "synchronous", "OFF");
    let _ = conn.pragma_update(None, "journal_mode", "MEMORY");
    let _ = conn.pragma_update(None, "cache_size", -131072i64); // 128MB page cache
    let _ = conn.execute("DROP INDEX IF EXISTS files_name", []);
}

pub fn bulk_end(conn: &Connection) {
    let _ = conn.execute("CREATE INDEX IF NOT EXISTS files_name ON files(name)", []);
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.pragma_update(None, "cache_size", -32768i64); // 常驻恢复为 32MB
    let _ = conn.execute_batch("PRAGMA shrink_memory;");
}

/// 把一卷的 MFT 枚举结果写入 files 表（先清该卷旧行再裸 INSERT）。
/// 调用方应已 bulk_begin。返回 (files, dirs)。
#[cfg(windows)]
pub fn insert_mft_entries(conn: &Connection, vol: &str, entries: &[crate::mft::Entry]) -> (usize, usize) {
    let mut files = 0usize;
    let mut dirs = 0usize;
    let Ok(tx) = conn.unchecked_transaction() else {
        return (0, 0);
    };
    {
        let prefix = format!("{}\\%", vol.trim_end_matches(':').to_uppercase());
        let _ = tx.execute(
            "DELETE FROM files WHERE path LIKE ? ESCAPE '\\'",
            params![prefix.replace('%', "\\%").replace('_', "\\_")],
        );
        let Ok(mut stmt) = tx.prepare(
            "INSERT INTO files
             (path, name, pinyin, pinyin_init, size, mtime, is_dir, indexed)
             VALUES (?,?,?,?,?,0,?,1)",
        ) else {
            return (0, 0);
        };
        for e in entries {
            let (py, init) = pinyin_of(&e.name);
            if stmt
                .execute(params![e.path, e.name, py, init, e.size as i64, e.is_dir as i64])
                .is_ok()
            {
                if e.is_dir {
                    dirs += 1;
                } else {
                    files += 1;
                }
            }
        }
    }
    let _ = tx.commit();
    (files, dirs)
}

/// 用 MFT/USN 枚举全盘文件名索引（Windows，需管理员）。
/// 返回 (stats, degraded)——degraded 是未能用 MFT 枚举的盘，调用方应降级目录遍历。
/// MFT 拿不到精确 mtime 与内容，只建文件名/拼音/目录索引，mtime 固定为 0。
#[cfg(windows)]
pub fn build_mft_index(
    conn: &Connection,
    volumes: Option<Vec<String>>,
) -> (ScanStats, Vec<String>) {
    let mut stats = ScanStats::default();
    let mut degraded = Vec::new();
    let volumes = volumes.unwrap_or_else(crate::mft::list_ntfs_volumes);
    bulk_begin(conn);
    for vol in volumes {
        match crate::mft::enumerate_volume(&vol) {
            None => degraded.push(vol),
            Some((entries, _frn_map, _journal)) => {
                let (f, d) = insert_mft_entries(conn, &vol, &entries);
                if f + d == 0 {
                    degraded.push(vol);
                } else {
                    stats.files += f;
                    stats.dirs += d;
                    stats.indexed += f + d;
                }
            }
        }
    }
    bulk_end(conn);
    (stats, degraded)
}
