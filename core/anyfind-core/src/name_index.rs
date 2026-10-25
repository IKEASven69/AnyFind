//! 内存文件名索引（Everything 式设计）。
//!
//! 数据依据（docs/bench-001.md）：SQLite LIKE 在 1.39M 行全盘语料上 750-920ms，
//! 超 <50ms 预算 15 倍；FTS5 trigram 只覆盖 ≥3 字符。名字驻留内存（~35MB/1.4M 条），
//! 子串匹配全词长覆盖，USN 增量同步。路径不驻留——命中后按 rowid 回查 files 表。
//!
//! 持久化：names.bin（自描述二进制，长度前缀），冷启动直接装载，避免扫库重建。

use crate::dsl::{self};
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct NameEntry {
    pub rowid: i64,
    pub name_lower: String,
    pub is_dir: bool,
    pub size: i64,
}

#[derive(Debug, Default)]
pub struct NameIndex {
    pub entries: Vec<NameEntry>,
}

#[derive(Debug, Clone)]
pub struct NameMatch {
    pub rowid: i64,
    pub score: f64,
    pub is_dir: bool,
    pub size: i64,
}

/// 名字匹配分（与 search::file_score 同序，但只用名字列；路径分由回查后补）。
fn score_name(q: &str, name_lower: &str) -> Option<f64> {
    let ql = q.to_lowercase();
    let stem = match name_lower.rfind('.') {
        Some(i) if i > 0 => &name_lower[..i],
        _ => name_lower,
    };
    if name_lower == ql || stem == ql {
        return Some(1.0);
    }
    if name_lower.starts_with(&ql) || stem.starts_with(&ql) {
        return Some(0.9);
    }
    if name_lower.contains(&ql) {
        return Some(0.75);
    }
    None
}

impl NameIndex {
    /// 从 files 表全量装载（lower 在 Rust 侧做，比 SQL lower() 快数倍）。
    /// 大库装载慢（1.4M 条约 2-4s）；冷启动应走 load_blob，本函数用于索引重建后。
    pub fn load_from_db(conn: &Connection) -> rusqlite::Result<Self> {
        let mut stmt = conn.prepare("SELECT id, name, is_dir, size FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut entries = Vec::new();
        for r in rows.flatten() {
            entries.push(NameEntry {
                rowid: r.0,
                name_lower: r.1.to_lowercase(),
                is_dir: r.2,
                size: r.3,
            });
        }
        Ok(NameIndex { entries })
    }

    pub fn upsert(&mut self, rowid: i64, name: &str, is_dir: bool, size: i64) {
        let name_lower = name.to_lowercase();
        match self.entries.binary_search_by_key(&rowid, |e| e.rowid) {
            Ok(i) => self.entries[i] = NameEntry { rowid, name_lower, is_dir, size },
            Err(i) => self.entries.insert(i, NameEntry { rowid, name_lower, is_dir, size }),
        }
    }

    pub fn remove(&mut self, rowid: i64) {
        if let Ok(i) = self.entries.binary_search_by_key(&rowid, |e| e.rowid) {
            self.entries.remove(i);
        }
    }

    /// 搜索：所有肯定词都命中名字（AND）；NOT/OR 由 expr 在名字上复核。
    /// 大索引（>20 万条）自动 rayon 并行。返回按 (score, is_dir, size) 排序的 top `limit`。
    pub fn search(&self, query: &dsl::Query, limit: usize) -> Vec<NameMatch> {
        let Some(expr) = &query.expr else { return Vec::new() };
        let words = dsl::positive_terms(query);
        if words.is_empty() {
            return Vec::new();
        }
        let word_lowers: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let match_one = |e: &NameEntry| -> Option<NameMatch> {
            if !word_lowers.iter().all(|w| e.name_lower.contains(w.as_str())) {
                return None;
            }
            if !dsl::text_matches_raw(expr, &e.name_lower) {
                return None;
            }
            let score = word_lowers
                .iter()
                .filter_map(|w| score_name(w, &e.name_lower))
                .fold(0.0f64, f64::max);
            Some(NameMatch { rowid: e.rowid, score, is_dir: e.is_dir, size: e.size })
        };
        let mut out: Vec<NameMatch> = if self.entries.len() > 200_000 {
            use rayon::prelude::*;
            self.entries.par_iter().filter_map(match_one).collect()
        } else {
            self.entries.iter().filter_map(match_one).collect()
        };
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.is_dir.cmp(&a.is_dir))
                .then_with(|| b.size.cmp(&a.size))
        });
        out.truncate(limit);
        out
    }

    // ---------- names.bin 持久化 ----------
    // 格式: magic u32 (0x41464E49 "AFNI") | count u32 |
    //       每条: rowid i64 | flags u8 (bit0=is_dir) | size i64 | name_len u32 | name bytes

    pub fn save_blob(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut buf = Vec::with_capacity(self.entries.len() * 32);
        buf.write_all(&0x41464E49u32.to_le_bytes())?;
        buf.write_all(&(self.entries.len() as u32).to_le_bytes())?;
        for e in &self.entries {
            buf.write_all(&e.rowid.to_le_bytes())?;
            buf.write_all(&[e.is_dir as u8])?;
            buf.write_all(&e.size.to_le_bytes())?;
            let b = e.name_lower.as_bytes();
            buf.write_all(&(b.len() as u32).to_le_bytes())?;
            buf.write_all(b)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn load_blob(path: &std::path::Path) -> Option<Self> {
        let buf = std::fs::read(path).ok()?;
        if buf.len() < 8 || u32::from_le_bytes(buf[0..4].try_into().ok()?) != 0x41464E49 {
            return None;
        }
        let count = u32::from_le_bytes(buf[4..8].try_into().ok()?) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut off = 8usize;
        for _ in 0..count {
            if off + 21 > buf.len() {
                return None;
            }
            let rowid = i64::from_le_bytes(buf[off..off + 8].try_into().ok()?);
            let is_dir = buf[off + 8] & 1 != 0;
            let size = i64::from_le_bytes(buf[off + 9..off + 17].try_into().ok()?);
            let nlen = u32::from_le_bytes(buf[off + 17..off + 21].try_into().ok()?) as usize;
            off += 21;
            if off + nlen > buf.len() {
                return None;
            }
            let name_lower = String::from_utf8_lossy(&buf[off..off + nlen]).into_owned();
            off += nlen;
            entries.push(NameEntry { rowid, name_lower, is_dir, size });
        }
        entries.sort_by_key(|e| e.rowid);
        Some(NameIndex { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_and_score() {
        let mut idx = NameIndex::default();
        idx.upsert(1, "nginx.conf", false, 100);
        idx.upsert(2, "my-nginx-backup.conf", false, 200);
        idx.upsert(3, "nginx", true, 0);
        idx.upsert(4, "unrelated.txt", false, 999);
        let q = dsl::parse("nginx");
        let hits = idx.search(&q, 10);
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].rowid, 3, "精确匹配 1.0 且目录优先");
        assert_eq!(hits[1].rowid, 1, "前缀 0.9");
        let q2 = dsl::parse("nginx -backup");
        let hits2 = idx.search(&q2, 10);
        assert!(hits2.iter().all(|h| h.rowid != 2));
    }

    #[test]
    fn blob_roundtrip() {
        let mut idx = NameIndex::default();
        idx.upsert(2, "中文名.md", false, 5);
        idx.upsert(1, "a.txt", true, 0);
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("names.bin");
        idx.save_blob(&p).unwrap();
        let back = NameIndex::load_blob(&p).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.entries[0].rowid, 1);
        assert_eq!(back.entries[1].name_lower, "中文名.md");
    }
}
