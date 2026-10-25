//! adapter: Pi
//! ~/.pi/agent/sessions/<路径转义>/<时间戳>_<uuid>.jsonl
//! type=session 给 cwd；type=message 的 message.content[] 为文本块。

use crate::message::*;
use crate::paths;
use crate::util;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub fn discover() -> Vec<SessionRef> {
    let mut refs = Vec::new();
    let Ok(dirs) = std::fs::read_dir(paths::pi_root()) else {
        return refs;
    };
    for d in dirs.flatten() {
        if !d.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let Some((mtime, size)) = stat_meta(&p) else { continue };
            let base = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let session_id = util::extract_uuid(&base).unwrap_or(base);
            refs.push(SessionRef {
                agent: "pi",
                session_id,
                path: p.clone(),
                cwd: cwd_from_head(&read_head(&p, 65536)),
                mtime,
                size,
                title: None,
            });
        }
    }
    refs
}

pub fn parse(path: &Path) -> Parsed {
    let mut out = Parsed::default();
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return out,
    };
    for line in BufReader::new(f).lines() {
        let Ok(line) = line else { continue };
        if !line.contains("\"type\":\"message\"")
            && !line.contains("\"type\": \"message\"")
            && !line.contains("\"type\":\"session\"")
            && !line.contains("\"type\": \"session\"")
        {
            continue;
        }
        let Ok(e) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let etype = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let ts = e
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if etype == "session" {
            if let Some(c) = e.get("cwd").and_then(|c| c.as_str()) {
                out.cwd = c.to_string();
            }
            continue;
        }
        if etype != "message" {
            continue;
        }
        let Some(msg) = e.get("message").filter(|m| m.is_object()) else {
            continue;
        };
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = join_text_blocks(msg.get("content").unwrap_or(&serde_json::Value::Null), &["text"]);
        if !text.is_empty() {
            out.messages.push(Message { role: role.to_string(), ts, text });
        }
    }
    out.title = first_user_title(&out.messages, 80);
    out
}
