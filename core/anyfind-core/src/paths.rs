//! 路径与数据目录。小型配置留在用户配置目录，大型索引默认放非系统盘。

use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 配置目录只占少量空间；索引不要默认放在这里。
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("anyfind")
}

fn parse_data_dir(text: &str) -> Option<PathBuf> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let path = value.get("data_dir")?.as_str()?.trim();
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

/// 默认选第一个非系统盘的 AnyFindData；只有系统盘时才回退到用户目录。
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let system = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into());
        for letter in b'C'..=b'Z' {
            let drive = format!("{}:", letter as char);
            if drive.eq_ignore_ascii_case(&system) {
                continue;
            }
            let root = PathBuf::from(format!("{drive}/"));
            if root.is_dir() {
                return root.join("AnyFindData");
            }
        }
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("anyfind");
    }
    home().join(".local").join("share").join("anyfind")
}

/// 优先级：ANYFIND_DATA_DIR > config.json 的 data_dir > 非系统盘默认目录。
pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("ANYFIND_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::fs::read_to_string(config_dir().join("config.json"))
                .ok()
                .and_then(|text| parse_data_dir(&text))
        })
        .unwrap_or_else(default_data_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn db_path() -> PathBuf {
    data_dir().join("index.db")
}

#[cfg(test)]
mod tests {
    use super::parse_data_dir;

    #[test]
    fn data_dir_config_must_be_absolute() {
        assert_eq!(
            parse_data_dir(r#"{"data_dir":"D:/AnyFindData"}"#),
            Some(std::path::PathBuf::from("D:/AnyFindData"))
        );
        assert_eq!(parse_data_dir(r#"{"data_dir":"relative/path"}"#), None);
        assert_eq!(parse_data_dir("not json"), None);
    }
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
    home()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("storage")
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
