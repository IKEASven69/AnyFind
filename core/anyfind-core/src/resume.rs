//! resume 命令生成与终端拉起（契约 /api/resume 语义）。

use crate::util;
use rusqlite::{params, Connection};

pub fn resume_command(agent: &str, session_id: &str, _cwd: Option<&str>) -> Result<String, String> {
    if session_id.is_empty() {
        return Err("缺少 session_id".into());
    }
    match agent {
        "claude-code" => Ok(format!("claude --resume {session_id}")),
        "codex" => Ok(format!("codex resume {session_id}")),
        "dsh" => Ok(format!("dsh tui --resume {session_id}")),
        "opencode" => Ok(format!("opencode --session {session_id}")),
        "pi" => Ok(format!("pi --session {session_id}")),
        "cursor" => Err("cursor 无 CLI resume 能力（或未探测到本地会话存储）".into()),
        other => Err(format!("未知 harness: {other}")),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMeta {
    pub agent: String,
    pub session_id: String,
    pub title: Option<String>,
    pub path: Option<String>,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub mtime: Option<String>,
}

/// 按 session_id 查会话元信息。支持 'agent:id' 限定前缀。
pub fn find_session(conn: &Connection, session_id: &str) -> Option<SessionMeta> {
    let pick = |r: &rusqlite::Row| -> rusqlite::Result<SessionMeta> {
        Ok(SessionMeta {
            agent: r.get(0)?,
            session_id: r.get(1)?,
            title: r.get(2)?,
            path: r.get(3)?,
            cwd: r.get(4)?,
            project: r.get(5)?,
            mtime: util::iso_ts(r.get::<_, Option<f64>>(6)?),
        })
    };
    if let Some((agent, sid)) = session_id.split_once(':') {
        if let Ok(m) = conn.query_row(
            "SELECT agent, session_id, title, path, cwd, project, mtime
             FROM sessions WHERE agent=? AND session_id=?",
            params![agent, sid],
            pick,
        ) {
            return Some(m);
        }
    }
    conn.query_row(
        "SELECT agent, session_id, title, path, cwd, project, mtime
         FROM sessions WHERE session_id=? ORDER BY mtime DESC LIMIT 1",
        params![session_id],
        pick,
    )
    .ok()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub agent: String,
    pub title: Option<String>,
    pub project: Option<String>,
    pub cwd: Option<String>,
    pub path: Option<String>,
    pub mtime: Option<String>,
    pub messages: Vec<crate::message::Message>,
}

/// 会话完整元信息 + 按消息分段的原文（契约 /api/session/{id}）。
pub fn get_session_messages(
    conn: &Connection,
    session_id: &str,
    max_messages: usize,
) -> Option<SessionDetail> {
    let meta = find_session(conn, session_id)?;
    let mut messages = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT m.role, m.ts, m.text FROM session_messages m
         JOIN sessions s ON s.id = m.session_rowid
         WHERE s.agent=? AND s.session_id=? ORDER BY m.id LIMIT ?",
    ) {
        if let Ok(rows) = stmt.query_map(
            params![meta.agent, meta.session_id, max_messages as i64],
            |r| {
                Ok(crate::message::Message {
                    role: r.get(0)?,
                    ts: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    text: r.get(2)?,
                })
            },
        ) {
            messages.extend(rows.flatten());
        }
    }
    Some(SessionDetail {
        session_id: meta.session_id,
        agent: meta.agent,
        title: meta.title,
        project: meta.project,
        cwd: meta.cwd,
        path: meta.path,
        mtime: meta.mtime,
        messages,
    })
}

/// 拉起终端窗口执行命令（detached）。Windows Terminal 优先，回退 conhost。
/// 返回 (ok, via, error)。
#[cfg(windows)]
pub fn launch_terminal(command: &str, cwd: &str) -> (bool, String, Option<String>) {
    if !cwd.is_empty() && !std::path::Path::new(cwd).is_dir() {
        return (false, String::new(), Some(format!("工作目录不存在: {cwd}")));
    }
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    let wt_candidates: Vec<std::path::PathBuf> = [
        local.map(|l| l.join("Microsoft").join("WindowsApps").join("wt.exe")),
        Some(std::path::PathBuf::from(r"C:\Program Files\WindowsApps\wt.exe")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let wt = wt_candidates.into_iter().find(|p| p.is_file());
    const DETACHED: u32 = 0x00000008; // DETACHED_PROCESS
    if let Some(wt) = wt {
        let mut cmd = Command::new(wt);
        cmd.arg("-d").arg(if cwd.is_empty() { "." } else { cwd })
            .arg("cmd").arg("/k").arg(command)
            .creation_flags(DETACHED);
        if !cwd.is_empty() {
            cmd.current_dir(cwd);
        }
        return match cmd.spawn() {
            Ok(_) => (true, "Windows Terminal".into(), None),
            Err(e) => (false, String::new(), Some(e.to_string())),
        };
    }
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/c", "start", "anyfind resume", "/D", if cwd.is_empty() { "." } else { cwd },
              "cmd", "/k", command])
        .creation_flags(DETACHED);
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }
    match cmd.spawn() {
        Ok(_) => (true, "conhost".into(), None),
        Err(e) => (false, String::new(), Some(e.to_string())),
    }
}

#[cfg(not(windows))]
pub fn launch_terminal(_command: &str, _cwd: &str) -> (bool, String, Option<String>) {
    (false, String::new(), Some("spawn 目前仅支持 Windows".into()))
}
