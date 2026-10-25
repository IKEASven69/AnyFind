//! adapter: dsh (DeepSeek Harness)
//! ~/.dsh/sessions/<--cwd转义-->/session-<uuid>/session[.vN].jsonl[.zstd]
//! 每个 session 目录取最高版本日志（session.v4 > v3 > v2 > 无后缀），同版本偏好 .zstd。
//! 首行 type=session 给 id/cwd；user/message、assistant/message 是权威消息
//! （*-chunks 是流式增量，跳过）；session/title 给标题。time 为 epoch 毫秒。

use crate::message::*;
use crate::paths;
use crate::util;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// 每个 session 目录取最高版本日志文件。
fn dsh_files() -> Vec<PathBuf> {
    let root = paths::dsh_root();
    let mut out = Vec::new();
    let Ok(esc_dirs) = std::fs::read_dir(&root) else {
        return out;
    };
    for esc in esc_dirs.flatten() {
        if !esc.path().is_dir() {
            continue;
        }
        let Ok(ses_dirs) = std::fs::read_dir(esc.path()) else {
            continue;
        };
        for ses in ses_dirs.flatten() {
            let ses_dir = ses.path();
            if !ses_dir.is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&ses_dir) else {
                continue;
            };
            // (version, prefer_zstd, filename)
            let mut best: Option<(u32, u32, String)> = None;
            for f in files.flatten() {
                let fn_ = f.file_name().to_string_lossy().into_owned();
                if let Some(v) = parse_session_filename(&fn_) {
                    if best.as_ref().is_none_or(|b| v > *b) {
                        best = Some(v);
                    }
                }
            }
            if let Some((_, _, fn_)) = best {
                out.push(ses_dir.join(fn_));
            }
        }
    }
    out
}

/// 匹配 session(?:\.v(\d+))?\.jsonl(\.zstd)? → (version, zstd偏好, filename)
fn parse_session_filename(fn_: &str) -> Option<(u32, u32, String)> {
    let rest = fn_.strip_prefix("session")?;
    let (rest, ver) = if let Some(r) = rest.strip_prefix(".v") {
        let digits: String = r.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        (r[digits.len()..].to_string(), digits.parse::<u32>().ok()?)
    } else {
        (rest.to_string(), 1)
    };
    let zstd = if rest == ".jsonl" {
        0
    } else if rest == ".jsonl.zstd" {
        1
    } else {
        return None;
    };
    Some((ver, zstd, fn_.to_string()))
}

fn open_reader(path: &Path) -> Option<Box<dyn BufRead>> {
    let f = std::fs::File::open(path).ok()?;
    if path.extension().and_then(|s| s.to_str()) == Some("zstd") {
        let dec = zstd::stream::read::Decoder::new(f).ok()?;
        Some(Box::new(BufReader::new(dec)))
    } else {
        Some(Box::new(BufReader::new(f)))
    }
}

pub fn discover() -> Vec<SessionRef> {
    let mut refs = Vec::new();
    for path in dsh_files() {
        let Some((mtime, size)) = stat_meta(&path) else { continue };
        let mut sid = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !sid.starts_with("session-") {
            sid = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            // 去掉 .jsonl（双后缀时 file_stem 只去一层）
            if let Some(s) = sid.strip_suffix(".jsonl") {
                sid = s.to_string();
            }
        }
        refs.push(SessionRef {
            agent: "dsh",
            session_id: sid,
            path,
            cwd: String::new(), // 首行 session 事件才有权威 cwd，parse 时取
            mtime,
            size,
            title: None,
        });
    }
    refs
}

pub fn parse(path: &Path) -> Parsed {
    let mut out = Parsed::default();
    let Some(reader) = open_reader(path) else {
        return out;
    };
    let mut sink = reader;
    let mut line = String::new();
    loop {
        line.clear();
        // 容错 UTF-8：read_line 在坏字节上报错，损失该行，不崩
        match sink.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                continue;
            }
        }
        let l = line.trim_end();
        if !l.contains("user/message")
            && !l.contains("assistant/message")
            && !l.contains("session/title")
            && !l.contains("\"type\": \"session\"")
            && !l.contains("\"type\":\"session\"")
        {
            continue;
        }
        let Ok(e) = serde_json::from_str::<serde_json::Value>(l) else {
            continue;
        };
        let etype = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let mut ts = e
            .get("time")
            .and_then(|t| t.as_f64())
            .map(util::iso_ms)
            .unwrap_or_default();
        let data = e.get("data").cloned().unwrap_or(serde_json::Value::Null);
        match etype {
            "session" => {
                if let Some(c) = e.get("cwd").and_then(|c| c.as_str()) {
                    out.cwd = c.to_string();
                }
                if ts.is_empty() {
                    ts = e
                        .get("createdAt")
                        .and_then(|t| t.as_f64())
                        .map(util::iso_ms)
                        .unwrap_or_default();
                }
            }
            "session/title" => {
                if let Some(t) = data.get("title").and_then(|t| t.as_str()) {
                    out.title = Some(t.to_string());
                }
            }
            "user/message" if data.is_object() => {
                let text = join_text_blocks(data.get("content").unwrap_or(&serde_json::Value::Null), &["text"]);
                if !text.is_empty() {
                    out.messages.push(Message { role: "user".into(), ts, text });
                }
            }
            "assistant/message" if data.is_object() => {
                if let Some(msg) = data.get("message").filter(|m| m.is_object()) {
                    let text = join_text_blocks(
                        msg.get("content").unwrap_or(&serde_json::Value::Null),
                        &["text"],
                    );
                    if !text.is_empty() {
                        out.messages.push(Message { role: "assistant".into(), ts, text });
                    }
                }
            }
            _ => {}
        }
    }
    if out.title.is_none() {
        out.title = first_user_title(&out.messages, 80);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_session_filename;

    #[test]
    fn dsh_filename_versions() {
        assert_eq!(
            parse_session_filename("session.jsonl"),
            Some((1, 0, "session.jsonl".into()))
        );
        assert_eq!(
            parse_session_filename("session.v3.jsonl.zstd"),
            Some((3, 1, "session.v3.jsonl.zstd".into()))
        );
        assert_eq!(
            parse_session_filename("session.v4.jsonl"),
            Some((4, 0, "session.v4.jsonl".into()))
        );
        assert_eq!(parse_session_filename("other.jsonl"), None);
        assert_eq!(parse_session_filename("session.v.jsonl"), None);
        // 同版本偏好 .zstd
        assert!(parse_session_filename("session.jsonl.zstd").unwrap()
            > parse_session_filename("session.jsonl").unwrap());
        assert!(parse_session_filename("session.v4.jsonl").unwrap()
            > parse_session_filename("session.v3.jsonl.zstd").unwrap());
    }
}
