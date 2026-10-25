//! adapter: Cursor（探测占位）
//! Cursor 无稳定的本地 jsonl 会话存储（聊天在 workspaceStorage 的 state.vscdb），
//! 保留探测接口（见 adapters::cursor_probe），发现候选目录即报告。

use crate::message::{Parsed, SessionRef};
use std::path::Path;

pub fn discover() -> Vec<SessionRef> {
    Vec::new()
}

pub fn parse(_path: &Path) -> Parsed {
    Parsed::default()
}
