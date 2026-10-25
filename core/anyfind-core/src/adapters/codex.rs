//! adapter: Codex
//! ~/.codex/sessions/**/*.jsonl（rollout-*.jsonl）。
//! session_meta 给 cwd/id；response_item(payload.type=message) 是权威消息流，
//! event_msg 为 UI 事件跳过。

use crate::message::*;
use crate::paths;
use crate::util;
use std::io::{BufRead, BufReader};
use std::path::Path;
use walkdir::WalkDir;

pub fn discover() -> Vec<SessionRef> {
    let mut refs = Vec::new();
    for e in WalkDir::new(paths::codex_root()).follow_links(true).into_iter().flatten() {
        let p = e.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some((mtime, size)) = stat_meta(p) else { continue };
        let base = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let session_id = util::extract_uuid(&base).unwrap_or_else(|| {
            p.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        refs.push(SessionRef {
            agent: "codex",
            session_id,
            path: p.to_path_buf(),
            cwd: cwd_from_head(&read_head(p, 8192)),
            mtime,
            size,
            title: None,
        });
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
        if !line.contains("\"session_meta\"") && !line.contains("\"response_item\"") {
            continue;
        }
        let Ok(e) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let ts = e
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let etype = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = e.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        if etype == "session_meta" {
            if let Some(c) = payload.get("cwd").and_then(|c| c.as_str()) {
                out.cwd = c.to_string();
            }
            continue;
        }
        if etype != "response_item" || !payload.is_object() {
            continue;
        }
        if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue; // function_call 等工具记录不进消息流
        }
        let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue; // system/developer 基线提示是噪声
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(arr) = payload.get("content").and_then(|c| c.as_array()) {
            for c in arr {
                if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
        let text = parts.join("\n").trim().to_string();
        if !text.is_empty() {
            out.messages.push(Message { role: role.to_string(), ts, text });
        }
    }
    out.title = first_user_title(&out.messages, 80);
    out
}
