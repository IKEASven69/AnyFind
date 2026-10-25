//! adapter: opencode
//! ~/.local/share/opencode/storage 三层文件存储：
//! session/<projectID>/ses_*.json（directory/title）
//! → message/<ses>/msg_*.json（role/time）
//! → part/<msg>/prt_*.json（text 块）。

use crate::message::*;
use crate::paths;
use crate::util;
use std::path::{Path, PathBuf};

fn load_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn session_files() -> Vec<PathBuf> {
    let dir = paths::opencode_root().join("session");
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(&dir) else {
        return out;
    };
    for prj in projects.flatten() {
        let Ok(files) = std::fs::read_dir(prj.path()) else {
            continue;
        };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.starts_with("ses_") && name.ends_with(".json") {
                out.push(f.path());
            }
        }
    }
    out
}

fn glob_prefixed(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(files) = std::fs::read_dir(dir) else {
        return out;
    };
    for f in files.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        if name.starts_with(prefix) && name.ends_with(".json") {
            out.push(f.path());
        }
    }
    out
}

pub fn discover() -> Vec<SessionRef> {
    let mut refs = Vec::new();
    for path in session_files() {
        let Some((mtime, size)) = stat_meta(&path) else { continue };
        let meta = load_json(&path).unwrap_or(serde_json::Value::Null);
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        refs.push(SessionRef {
            agent: "opencode",
            session_id,
            path,
            cwd: meta
                .get("directory")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            mtime,
            size,
            title: meta
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        });
    }
    refs
}

pub fn parse(path: &Path) -> Parsed {
    let mut out = Parsed::default();
    let Some(meta) = load_json(path) else {
        return out;
    };
    let Some(sid) = meta.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
        return out;
    };
    out.cwd = meta
        .get("directory")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    out.title = meta
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let root = paths::opencode_root();
    let mut msg_objs: Vec<(f64, serde_json::Value)> = Vec::new();
    for mp in glob_prefixed(&root.join("message").join(&sid), "msg_") {
        if let Some(m) = load_json(&mp) {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if (role == "user" || role == "assistant")
                && m.get("id").and_then(|v| v.as_str()).is_some()
            {
                let created = m
                    .get("time")
                    .and_then(|t| t.get("created"))
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                msg_objs.push((created, m));
            }
        }
    }
    msg_objs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    for (created, m) in msg_objs {
        let ts = util::iso_ms(created);
        let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let mut parts: Vec<String> = Vec::new();
        for pp in glob_prefixed(&root.join("part").join(mid), "prt_") {
            if let Some(p) = load_json(&pp) {
                if p.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                        parts.push(t.to_string());
                    }
                }
            }
        }
        let text = parts.join("\n").trim().to_string();
        if !text.is_empty() {
            out.messages.push(Message {
                role: m.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                ts,
                text,
            });
        }
    }
    if out.title.is_none() {
        out.title = first_user_title(&out.messages, 80);
    }
    out
}
