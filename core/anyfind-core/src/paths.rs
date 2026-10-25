//! 路径与数据目录。索引数据放 %APPDATA%/anyfind/（Windows）或 ~/.config/anyfind/。

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 索引数据目录：%APPDATA%/anyfind/，不存在则创建。
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    let dir = base.join("anyfind");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn db_path() -> PathBuf {
    data_dir().join("index.db")
}

// ---- 各家 harness 日志根目录（与 Python session_index 一致）----

pub fn claude_root() -> PathBuf {
    home().join(".claude").join("projects")
}
pub fn codex_root() -> PathBuf {
    home().join(".codex").join("sessions")
}
pub fn dsh_root() -> PathBuf {
    home().join(".dsh").join("sessions")
}
pub fn opencode_root() -> PathBuf {
    home().join(".local").join("share").join("opencode").join("storage")
}
pub fn pi_root() -> PathBuf {
    home().join(".pi").join("agent").join("sessions")
}
pub fn cursor_candidates() -> Vec<PathBuf> {
    let mut v = vec![home().join(".cursor")];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        v.push(
            PathBuf::from(appdata)
                .join("Cursor")
                .join("User")
                .join("workspaceStorage"),
        );
    }
    v
}
