//! adapter: Claude Code
//! ~/.claude/projects/**/*.jsonl；行级 type=user/assistant，message.content 为
//! 字符串或 block 数组（text / thinking / tool_use / tool_result）。cwd 在行级字段。

use crate::message::*;
use crate::paths;
use std::io::{BufRead, BufReader};
use std::path::Path;
use walkdir::WalkDir;

pub fn discover() -> Vec<SessionRef> {
    let mut refs = Vec::new();
    for e in WalkDir::new(paths::claude_root()).follow_links(true).into_iter().flatten() {
        let p = e.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some((mtime, size)) = stat_meta(p) else { continue };
        let session_id = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        refs.push(SessionRef {
            agent: "claude-code",
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
        // 行级预过滤：绝大多数行（progress/file-history 等）直接跳过
        if !line.contains("\"type\":\"user\"")
            && !line.contains("\"type\":\"assistant\"")
            && !line.contains("\"type\": \"user\"")
            && !line.contains("\"type\": \"assistant\"")
        {
            continue;
        }
        let Ok(e) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(msg) = e.get("message").filter(|m| m.is_object()) else {
            continue;
        };
        if out.cwd.is_empty() {
            if let Some(c) = e.get("cwd").and_then(|c| c.as_str()) {
                out.cwd = c.to_string();
            }
        }
        let ts = e
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let etype = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if (etype == "user" && role == "user") || (etype == "assistant" && role == "assistant") {
            let text = join_text_blocks(msg.get("content").unwrap_or(&serde_json::Value::Null), &["text"]);
            if !text.is_empty() {
                out.messages.push(Message { role: role.to_string(), ts, text });
            }
        }
    }
    out.title = first_user_title(&out.messages, 80);
    out
}
