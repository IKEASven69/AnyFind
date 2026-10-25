//! 统一消息流与适配器公共类型。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SessionRef {
    pub agent: &'static str,
    pub session_id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub mtime: f64,
    pub size: u64,
    pub title: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    pub role: String,
    pub ts: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub cwd: String,
    pub title: Option<String>,
    pub messages: Vec<Message>,
}

/// 首条用户消息当标题；跳过环境注入消息（AGENTS.md / environment_context / caveat）。
pub fn first_user_title(messages: &[Message], n: usize) -> Option<String> {
    let is_inject = |t: &str| {
        t.starts_with("# AGENTS.md")
            || t.starts_with("<environment_context")
            || t.starts_with("<local-command-caveat>")
    };
    let mut fallback: Option<String> = None;
    for m in messages {
        if m.role != "user" || m.text.is_empty() {
            continue;
        }
        let flat: String = m.text.replace('\n', " ");
        let cut: String = flat.chars().take(n).collect();
        if is_inject(&m.text) {
            if fallback.is_none() {
                fallback = Some(cut);
            }
            continue;
        }
        return Some(cut);
    }
    fallback
}

/// content（JSON 字符串或 block 数组）→ 纯文本。只取指定 type 的块。
pub fn join_text_blocks(content: &serde_json::Value, block_types: &[&str]) -> String {
    match content {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Array(arr) => {
            let parts: Vec<&str> = arr
                .iter()
                .filter_map(|b| {
                    let t = b.get("type")?.as_str()?;
                    if block_types.contains(&t) {
                        b.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .filter(|s| !s.is_empty())
                .collect();
            parts.join("\n").trim().to_string()
        }
        _ => String::new(),
    }
}

/// 读文件头若干字节（UTF-8 容错）。
pub fn read_head(path: &std::path::Path, nbytes: usize) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; nbytes];
    let n = match std::fs::File::open(path) {
        Ok(mut f) => f.read(&mut buf).unwrap_or(0),
        Err(_) => return String::new(),
    };
    buf.truncate(n);
    String::from_utf8_lossy(&buf).into_owned()
}

/// 从文件头 JSON 片段里抠 "cwd" 字段（免整行解析的正则捷径）。
pub fn cwd_from_head(head: &str) -> String {
    let mut rest = head;
    loop {
        let Some(idx) = rest.find("\"cwd\"") else {
            return String::new();
        };
        rest = &rest[idx + 5..];
        let rest_trim = rest.trim_start();
        let Some(after) = rest_trim.strip_prefix(':') else {
            continue;
        };
        let after = after.trim_start();
        let Some(after) = after.strip_prefix('"') else {
            return String::new();
        };
        // 收集到未转义的收尾引号
        let mut out = String::new();
        let mut chars = after.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        out.push(match n {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    }
                }
                '"' => return out,
                c => out.push(c),
            }
        }
        return String::new();
    }
}

pub fn stat_meta(path: &std::path::Path) -> Option<(f64, u64)> {
    let st = std::fs::metadata(path).ok()?;
    let mtime = st
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    Some((mtime, st.len()))
}
