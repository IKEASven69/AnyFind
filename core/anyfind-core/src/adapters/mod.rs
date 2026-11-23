//! 六家 AI harness 会话适配器（移植自前身 Python 参考实现 session_index.py）。
//! 每家：discover() → SessionRef 列表；parse(path) → Parsed。
//! 解析容错：坏行跳过不崩；增量按 (mtime, size) 指纹（在 db 层）。

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod dsh;
pub mod opencode;
pub mod pi;

use crate::message::{Parsed, SessionRef};
use std::path::Path;

pub struct Adapter {
    pub name: &'static str,
    pub discover: fn() -> Vec<SessionRef>,
    pub parse: fn(&Path) -> Parsed,
}

pub const ADAPTERS: &[Adapter] = &[
    Adapter { name: "claude-code", discover: claude::discover, parse: claude::parse },
    Adapter { name: "codex", discover: codex::discover, parse: codex::parse },
    Adapter { name: "dsh", discover: dsh::discover, parse: dsh::parse },
    Adapter { name: "opencode", discover: opencode::discover, parse: opencode::parse },
    Adapter { name: "pi", discover: pi::discover, parse: pi::parse },
    Adapter { name: "cursor", discover: cursor::discover, parse: cursor::parse },
];

/// Cursor 探测占位：返回 (found_roots, note)。
pub fn cursor_probe() -> (Vec<String>, String) {
    let found: Vec<String> = crate::paths::cursor_candidates()
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let note = if found.is_empty() {
        "未探测到 Cursor 本地数据目录（可能未安装桌面端）。".to_string()
    } else {
        "Cursor 会话存储于 workspaceStorage/state.vscdb（二进制 SQLite），暂无 jsonl 适配；探测到目录但未实现解析。"
            .to_string()
    };
    (found, note)
}
