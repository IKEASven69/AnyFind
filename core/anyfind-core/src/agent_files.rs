//! agent 记忆 / 技能文件索引（移植自 filefind/agent_files_index.py）。
//! kind = skill | memory；项目级目录从 sessions 表的 cwd 推导，不另配扫描根。

use crate::db;
use crate::paths;
use crate::util;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MAX_TEXT: u64 = 1024 * 1024; // 单文件索引文本上限 1MB

/// (目录, 归属 harness)。不存在的目录跳过。
fn skill_roots() -> Vec<(PathBuf, Option<&'static str>)> {
    let h = paths::home();
    vec![
        (h.join(".claude").join("skills"), Some("claude-code")),
        (h.join(".codex").join("skills"), Some("codex")),
        (h.join(".config").join("opencode").join("skills"), Some("opencode")),
        (h.join(".local").join("share").join("opencode").join("skills"), Some("opencode")),
        (h.join(".cursor").join("skills"), Some("cursor")),
        (h.join(".pi").join("agent").join("skills"), Some("pi")),
    ]
}

/// 全局记忆/配置文件：(精确文件, 归属 harness)。AGENTS.md 跨 harness → None。
fn memory_sources() -> Vec<(PathBuf, Option<&'static str>)> {
    let h = paths::home();
    vec![
        (h.join(".claude").join("CLAUDE.md"), Some("claude-code")),
        (h.join(".claude").join("AGENTS.md"), None),
        (h.join(".codex").join("AGENTS.md"), None),
        (h.join(".codex").join("CLAUDE.md"), Some("claude-code")),
        (h.join(".config").join("opencode").join("AGENTS.md"), None),
        (h.join(".pi").join("agent").join("AGENTS.md"), None),
    ]
}

#[derive(Debug, Clone)]
pub struct AgentFileRef {
    pub kind: &'static str, // skill | memory
    pub agent: Option<&'static str>,
    pub path: PathBuf,
    pub name: String,
    pub project: Option<String>,
}

pub fn discover_agent_files(project_cwds: &[String]) -> Vec<AgentFileRef> {
    let mut out: Vec<AgentFileRef> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut add = |kind: &'static str, agent: Option<&'static str>, path: PathBuf, project: Option<String>| {
        let key = path.to_string_lossy().into_owned();
        if seen.contains(&key) || !path.is_file() {
            return;
        }
        seen.insert(key);
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(AgentFileRef { kind, agent, path, name, project });
    };

    // 全局 skills 目录：递归全部 .md
    for (root, agent) in skill_roots() {
        if !root.is_dir() {
            continue;
        }
        for e in WalkDir::new(root).follow_links(true).into_iter().flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("md"))
            {
                add("skill", agent, p.to_path_buf(), None);
            }
        }
    }

    // 全局记忆/配置文件（精确文件）
    for (path, agent) in memory_sources() {
        add("memory", agent, path, None);
    }
    // glob 型：~/.codex/memories/**/*.md、~/.codex/rules/*.md
    for dir in [
        paths::home().join(".codex").join("memories"),
        paths::home().join(".codex").join("rules"),
    ] {
        if !dir.is_dir() {
            continue;
        }
        for e in WalkDir::new(dir).follow_links(true).into_iter().flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("md"))
            {
                add("memory", Some("codex"), p.to_path_buf(), None);
            }
        }
    }

    // 项目级：CLAUDE.md / AGENTS.md / .claude/*.md / .claude/skills/**
    for cwd in project_cwds {
        let cwdp = Path::new(cwd);
        if cwd.is_empty() || !cwdp.is_dir() {
            continue;
        }
        let project = util::project_from_cwd(cwd);
        add("memory", Some("claude-code"), cwdp.join("CLAUDE.md"), project.clone());
        add("memory", None, cwdp.join("AGENTS.md"), project.clone());
        let claude_dir = cwdp.join(".claude");
        if claude_dir.is_dir() {
            if let Ok(files) = std::fs::read_dir(&claude_dir) {
                for f in files.flatten() {
                    let p = f.path();
                    if p.is_file()
                        && p.extension().and_then(|s| s.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("md"))
                    {
                        add("memory", Some("claude-code"), p, project.clone());
                    }
                }
            }
            let skills_dir = claude_dir.join("skills");
            if skills_dir.is_dir() {
                for e in WalkDir::new(skills_dir).follow_links(true).into_iter().flatten() {
                    let p = e.path();
                    if p.is_file()
                        && p.extension().and_then(|s| s.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("md"))
                    {
                        add("skill", Some("claude-code"), p.to_path_buf(), project.clone());
                    }
                }
            }
        }
    }
    out
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct AgentFilesStats {
    pub candidates: usize,
    pub projects: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
}

/// 全量 discover + 指纹增量更新。依赖 sessions 表推导项目目录。
pub fn build_agent_files_index(conn: &Connection) -> AgentFilesStats {
    let mut stats = AgentFilesStats::default();
    let cwds = db::distinct_cwds(conn, 1000);
    stats.projects = cwds.len();
    let candidates = discover_agent_files(&cwds);
    stats.candidates = candidates.len();

    let mut existing: std::collections::HashMap<String, (i64, f64, i64)> =
        std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT id, path, mtime, size FROM agent_files") {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?, r.get::<_, i64>(3)?))
        }) {
            for row in rows.flatten() {
                existing.insert(row.1, (row.0, row.2, row.3));
            }
        }
    }

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in &candidates {
        let key = c.path.to_string_lossy().into_owned();
        seen.insert(key.clone());
        let Some((mtime, size)) = crate::message::stat_meta(&c.path) else { continue };
        let cur = existing.get(&key).copied();
        if let Some((_, m, s)) = cur {
            if (m - mtime).abs() < 1e-3 && s == size as i64 {
                stats.skipped += 1;
                continue;
            }
        }
        let text = match std::fs::File::open(&c.path) {
            Ok(f) => {
                use std::io::Read;
                let mut buf = Vec::new();
                if f.take(MAX_TEXT).read_to_end(&mut buf).is_err() {
                    continue;
                }
                String::from_utf8_lossy(&buf).into_owned()
            }
            Err(_) => continue,
        };
        let res = if let Some((rowid, _, _)) = cur {
            conn.execute(
                "UPDATE agent_files SET kind=?, agent=?, name=?, project=?, mtime=?, size=?, text=?
                 WHERE id=?",
                params![c.kind, c.agent, c.name, c.project, mtime, size as i64, text, rowid],
            )
        } else {
            conn.execute(
                "INSERT INTO agent_files (kind, agent, path, name, project, mtime, size, text)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![c.kind, c.agent, key, c.name, c.project, mtime, size as i64, text],
            )
        };
        if res.is_ok() {
            stats.updated += 1;
        }
    }
    for (path, (rowid, _, _)) in &existing {
        if !seen.contains(path) {
            let _ = conn.execute("DELETE FROM agent_files WHERE id=?", params![rowid]);
            stats.removed += 1;
        }
    }
    stats
}
