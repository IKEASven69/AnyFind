//! file_ops：/api/file/* 端点的安全纯函数（docs/api-contract.md v0.2）。
//!
//! 覆盖可单测逻辑：Windows 保留名/非法字符/末尾点空格校验、噪声过滤、
//! 行分页、canonicalize 前缀 containment、符号链接深度 ≤ MAX_LINK_DEPTH。
//! I/O 仅出现在 canonicalize_limited（逐组件解析，最后 std::fs::canonicalize 归一）。

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// 符号链接最大解析深度（超过即拒绝）。
pub const MAX_LINK_DEPTH: u32 = 32;

/// 默认隐藏的噪声目录（大小写不敏感精确匹配）。
pub const NOISE_DIRS: &[&str] = &[
    ".git", "node_modules", "dist", "build", "coverage", ".idea", "target",
];

pub const LIST_MAX_DEFAULT: usize = 500;
pub const LIST_MAX_LIMIT: usize = 2000;
pub const READ_LINES_DEFAULT: usize = 30;
pub const READ_LINES_MAX: usize = 500;
/// read 大小上限：2MiB（超过 → 413）。
pub const READ_SIZE_MAX: u64 = 2 * 1024 * 1024;
/// read 二进制嗅探窗口：前 8KiB 含 NUL 即视为二进制。
pub const READ_SNIFF_BYTES: usize = 8 * 1024;
/// /api/file/op 单请求 paths 上限（超过 → 400）。
pub const OP_PATHS_MAX: usize = 128;

/// Windows 保留设备名（按第一个点前的主干判断，大小写不敏感）。
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// 新建/重命名目标名校验：非空、非 "."/".."、无控制与非法字符
/// （`<>:"/\|?*`）、不以点/空格结尾、非 Windows 保留名。
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name == "." || name == ".." {
        return Err(format!("非法名称: {name}"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("名称不能以点或空格结尾: {name}"));
    }
    for c in name.chars() {
        if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            return Err(format!("名称含非法字符: {name}"));
        }
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    if RESERVED_NAMES.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return Err(format!("Windows 保留名: {name}"));
    }
    Ok(())
}

/// 是否噪声目录名（大小写不敏感）。
pub fn is_noise(name: &str) -> bool {
    NOISE_DIRS.iter().any(|n| name.eq_ignore_ascii_case(n))
}

/// list 的 max 参数：解析失败 → 默认，越界 → 钳制到 1..=2000。
pub fn clamp_list_max(v: Option<&str>) -> usize {
    v.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(LIST_MAX_DEFAULT)
        .clamp(1, LIST_MAX_LIMIT)
}

/// read 的 lines 参数：解析失败 → 默认，钳制到 1..=500。
pub fn clamp_read_lines(v: Option<&str>) -> usize {
    v.and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(READ_LINES_DEFAULT)
        .clamp(1, READ_LINES_MAX)
}

/// read 的 offset 参数：0 起行号，解析失败/负数 → 0。
pub fn parse_offset(v: Option<&str>) -> usize {
    v.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0)
}

/// 前 8KiB（或全文件）含 NUL → 二进制。
pub fn is_binary_prefix(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(READ_SNIFF_BYTES)].contains(&0)
}

/// 行分页结果。
pub struct Page {
    pub lines: Vec<String>,
    pub total: usize,
    pub truncated: bool,
}

/// 按行分页：`\n` 切分、去行尾 `\r`、文件尾随换行不产生空行；
/// 从 `offset` 起取 `lines` 行；`truncated` = 窗口之后还有行。
pub fn paginate(content: &str, offset: usize, lines: usize) -> Page {
    let all = split_lines(content);
    let total = all.len();
    let start = offset.min(total);
    let end = offset.saturating_add(lines).min(total);
    Page {
        lines: all[start..end].iter().map(|s| (*s).to_string()).collect(),
        total,
        truncated: end < total,
    }
}

fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        v.pop();
    }
    v.iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// 目录条目（list 用）。
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<String>,
}

/// 噪声过滤（仅隐藏目录）+ 目录优先/同组按名排序（大小写不敏感）+ 截断。
/// 返回 (条目, truncated)。
pub fn prepare_entries(mut items: Vec<Entry>, show_noise: bool, max: usize) -> (Vec<Entry>, bool) {
    if !show_noise {
        items.retain(|e| !(e.is_dir && is_noise(&e.name)));
    }
    items.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let truncated = items.len() > max;
    items.truncate(max);
    (items, truncated)
}

/// 路径必须非空且绝对（Windows 盘符/UNC 由 Path 判定，反斜杠原生支持）。
pub fn require_absolute(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("缺少 path 参数".into());
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(format!("需要绝对路径: {raw}"))
    }
}

/// 前缀 containment（组件级比较，Windows 大小写不敏感，去 verbatim 前缀）。
/// 两侧应为 canonicalize 结果；`../` 无法越出 base。
pub fn is_within(base: &Path, child: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().to_lowercase();
                match s.strip_prefix("\\\\?\\") {
                    Some(rest) => rest.to_string(),
                    None => s,
                }
            })
            .collect()
    };
    let b = norm(base);
    let c = norm(child);
    c.len() >= b.len() && b.iter().zip(c.iter()).all(|(x, y)| x == y)
}

/// 拆出路径的根（盘符/UNC 前缀 + 根分隔符）与其余组件（跳过 "."）。
pub fn split_root(p: &Path) -> (PathBuf, Vec<OsString>) {
    let mut root = PathBuf::new();
    let mut rest = Vec::new();
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => root.push(c.as_os_str()),
            Component::CurDir => {}
            other => rest.push(other.as_os_str().to_os_string()),
        }
    }
    (root, rest)
}

/// 是否就是根路径（如 `D:\`、`/`、UNC 共享根）。
pub fn is_root_path(p: &Path) -> bool {
    let (root, rest) = split_root(p);
    !root.as_os_str().is_empty() && rest.is_empty()
}

/// 解析路径：词法归一（`.`/`..`）+ 逐组件跟随符号链接（深度 ≤ MAX_LINK_DEPTH）+
/// 存在性检查，最后 `std::fs::canonicalize` 归一（此时已无链接，不增加深度）。
/// 相对路径 / 不存在 / 深度超限 → Err（调用方映射 400/404）。
pub fn canonicalize_limited(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("需要绝对路径: {}", path.display()));
    }
    let (mut resolved, rest) = split_root(path);
    let mut queue: VecDeque<OsString> = rest.into();
    let mut hops: u32 = 0;
    while let Some(comp) = queue.pop_front() {
        let s = comp.to_string_lossy();
        if s == "." {
            continue;
        }
        if s == ".." {
            // base 已无链接，词法上弹即可；不越过根。
            if matches!(resolved.components().next_back(), Some(Component::Normal(_))) {
                resolved.pop();
            }
            continue;
        }
        let candidate = resolved.join(&comp);
        match std::fs::symlink_metadata(&candidate) {
            Ok(md) if md.file_type().is_symlink() => {
                hops += 1;
                if hops > MAX_LINK_DEPTH {
                    return Err(format!(
                        "符号链接深度超过 {MAX_LINK_DEPTH}: {}",
                        path.display()
                    ));
                }
                let target = std::fs::read_link(&candidate)
                    .map_err(|e| format!("读取符号链接失败 {}: {e}", candidate.display()))?;
                let (troot, trest) = if target.is_absolute() {
                    let (r, rest) = split_root(&target);
                    (Some(r), rest)
                } else {
                    (
                        None,
                        target
                            .components()
                            .map(|c| c.as_os_str().to_os_string())
                            .collect::<Vec<_>>(),
                    )
                };
                if let Some(r) = troot {
                    resolved = r;
                }
                let mut new_queue: VecDeque<OsString> = trest.into();
                new_queue.append(&mut queue);
                queue = new_queue;
            }
            Ok(_) => resolved = candidate,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("路径不存在: {}", candidate.display()));
            }
            Err(e) => return Err(format!("无法读取路径 {}: {e}", candidate.display())),
        }
    }
    std::fs::canonicalize(&resolved)
        .map_err(|e| format!("路径不可读 {}: {e}", resolved.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            is_dir,
            size: 0,
            mtime: None,
        }
    }

    // ---- validate_name ----

    #[test]
    fn validate_name_accepts_normal_names() {
        for n in [
            "file.txt",
            "新文件 2024",
            "a+b",
            ".gitignore",
            "COM10",
            "aux2",
            "report.final.pdf",
            "con1",
        ] {
            assert!(validate_name(n).is_ok(), "should accept: {n}");
        }
    }

    #[test]
    fn validate_name_rejects_reserved_and_illegal() {
        for n in [
            "", ".", "..", "CON", "con", "Prn", "AUX", "NUL", "COM1", "com9", "LPT1", "lpt9",
            "CON.txt", "nul.log", "a<b", "a>b", "a:b", "a\"b", "a/b", "a\\b", "a|b", "a?b",
            "a*b", "x\u{0}y",
        ] {
            assert!(validate_name(n).is_err(), "should reject: {n:?}");
        }
    }

    #[test]
    fn validate_name_rejects_trailing_dot_or_space() {
        assert!(validate_name("name.").is_err());
        assert!(validate_name("name ").is_err());
        assert!(validate_name("con .txt").is_err()); // 主干尾随空格 → 保留名
        assert!(validate_name("...").is_err());
    }

    // ---- 噪声 / 参数钳制 ----

    #[test]
    fn noise_matching_is_case_insensitive_exact() {
        for n in [".git", "NODE_MODULES", "Dist", "build", "coverage", ".IDEA", "target"] {
            assert!(is_noise(n), "noise: {n}");
        }
        for n in ["src", "dist2", "build.rs", "targets", ""] {
            assert!(!is_noise(n), "not noise: {n}");
        }
    }

    #[test]
    fn clamp_list_max_defaults_and_clamps() {
        assert_eq!(clamp_list_max(None), 500);
        assert_eq!(clamp_list_max(Some("10")), 10);
        assert_eq!(clamp_list_max(Some("0")), 1);
        assert_eq!(clamp_list_max(Some("9999")), 2000);
        assert_eq!(clamp_list_max(Some("abc")), 500);
    }

    #[test]
    fn clamp_read_lines_and_offset() {
        assert_eq!(clamp_read_lines(None), 30);
        assert_eq!(clamp_read_lines(Some("0")), 1);
        assert_eq!(clamp_read_lines(Some("1000")), 500);
        assert_eq!(clamp_read_lines(Some("x")), 30);
        assert_eq!(parse_offset(None), 0);
        assert_eq!(parse_offset(Some("-1")), 0);
        assert_eq!(parse_offset(Some("7")), 7);
    }

    // ---- 二进制嗅探 ----

    #[test]
    fn binary_prefix_checks_first_8kib() {
        assert!(!is_binary_prefix(b"hello"));
        assert!(!is_binary_prefix(b""));
        assert!(is_binary_prefix(b"ab\u{0}cd"));
        let mut v = vec![b'a'; READ_SNIFF_BYTES];
        v.push(0); // 第 8193 字节，窗口外
        assert!(!is_binary_prefix(&v));
        let mut v2 = vec![b'a'; READ_SNIFF_BYTES - 1];
        v2.push(0); // 恰好窗口内最后一个字节
        assert!(is_binary_prefix(&v2));
    }

    // ---- 分页 ----

    #[test]
    fn paginate_windows_and_crlf() {
        let p = paginate("a\nb\nc", 0, 2);
        assert_eq!(p.lines, vec!["a", "b"]);
        assert_eq!(p.total, 3);
        assert!(p.truncated);

        let p = paginate("a\nb\nc", 2, 10);
        assert_eq!(p.lines, vec!["c"]);
        assert!(!p.truncated);

        let p = paginate("a\nb\nc", 1, 1);
        assert_eq!(p.lines, vec!["b"]);
        assert!(p.truncated);

        let p = paginate("a\r\nb", 0, 10);
        assert_eq!(p.lines, vec!["a", "b"]);
        assert_eq!(p.total, 2);
    }

    #[test]
    fn paginate_edge_cases() {
        let p = paginate("", 0, 10);
        assert!(p.lines.is_empty());
        assert_eq!(p.total, 0);
        assert!(!p.truncated);

        let p = paginate("a\nb\n", 0, 10); // 尾随换行不产生空行
        assert_eq!(p.lines, vec!["a", "b"]);
        assert_eq!(p.total, 2);

        let p = paginate("\n", 0, 10); // 单换行 = 一个空行
        assert_eq!(p.lines, vec![""]);
        assert_eq!(p.total, 1);

        let p = paginate("a\nb\nc", 99, 10); // offset 越界
        assert!(p.lines.is_empty());
        assert!(!p.truncated);

        let p = paginate("a\n\nb", 0, 10);
        assert_eq!(p.lines, vec!["a", "", "b"]);
        assert_eq!(p.total, 3);
    }

    // ---- 列表过滤排序截断 ----

    #[test]
    fn prepare_entries_filters_noise_dirs_only() {
        let items = vec![
            entry("node_modules", true),
            entry("build", true),
            entry("build", false), // 同名文件不隐藏
            entry("src", true),
        ];
        let (out, tr) = prepare_entries(items, false, 100);
        let names: Vec<_> = out.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, vec![("src", true), ("build", false)]);
        assert!(!tr);

        // noise=1 全显示
        let items = vec![entry("node_modules", true), entry("src", true)];
        let (out, _) = prepare_entries(items, true, 100);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn prepare_entries_sorts_dirs_first_then_name() {
        let items = vec![
            entry("f.txt", false),
            entry("Bdir", true),
            entry("adir", true),
            entry("efile", false),
            entry("cfile", false),
        ];
        let (out, _) = prepare_entries(items, false, 100);
        let names: Vec<_> = out.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["adir", "Bdir", "cfile", "efile", "f.txt"]);
    }

    #[test]
    fn prepare_entries_truncates_and_flags() {
        let items = vec![entry("a", true), entry("b", true), entry("c", true)];
        let (out, tr) = prepare_entries(items, false, 2);
        assert_eq!(out.len(), 2);
        assert!(tr);

        let items = vec![entry("a", true), entry("b", true)];
        let (out, tr) = prepare_entries(items, false, 2);
        assert_eq!(out.len(), 2);
        assert!(!tr);
    }

    // ---- containment / 根 ----

    #[cfg(windows)]
    #[test]
    fn is_within_component_prefix_rules() {
        assert!(is_within(Path::new(r"D:\a"), Path::new(r"D:\a\b")));
        assert!(is_within(Path::new(r"D:\a"), Path::new(r"D:\a")));
        assert!(!is_within(Path::new(r"D:\a"), Path::new(r"D:\ab"))); // 字符串前缀 ≠ 组件前缀
        assert!(!is_within(Path::new(r"D:\a\b"), Path::new(r"D:\a")));
        assert!(is_within(Path::new(r"D:\A"), Path::new(r"d:\a\b"))); // 大小写不敏感
        assert!(is_within(
            Path::new(r"\\?\D:\a"),
            Path::new(r"D:\a\b")
        )); // verbatim 归一
        // ../ 逃逸在 canonicalize 后不可能：base=D:\a，child 不会是 D:\x
        assert!(!is_within(Path::new(r"D:\a"), Path::new(r"D:\x")));
    }

    #[cfg(not(windows))]
    #[test]
    fn is_within_component_prefix_rules_unix() {
        assert!(is_within(Path::new("/a"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a"), Path::new("/ab")));
        assert!(!is_within(Path::new("/a/b"), Path::new("/a")));
    }

    #[cfg(windows)]
    #[test]
    fn root_path_detection() {
        assert!(is_root_path(Path::new(r"D:\\")));
        assert!(is_root_path(Path::new(r"D:\.")));
        assert!(!is_root_path(Path::new(r"D:\\a")));
        assert!(!is_root_path(Path::new("")));
        assert!(!is_root_path(Path::new("rel")));
    }

    #[cfg(not(windows))]
    #[test]
    fn root_path_detection_unix() {
        assert!(is_root_path(Path::new("/")));
        assert!(!is_root_path(Path::new("/a")));
        assert!(!is_root_path(Path::new("")));
    }

    // ---- require_absolute ----

    #[cfg(windows)]
    #[test]
    fn require_absolute_windows_forms() {
        assert!(require_absolute(r"D:\coding").is_ok());
        assert!(require_absolute(r"d:/coding").is_ok());
        assert!(require_absolute(r"\\server\share\x").is_ok());
        assert!(require_absolute("").is_err());
        assert!(require_absolute("   ").is_err());
        assert!(require_absolute("rel\\path").is_err());
        assert!(require_absolute("C:foo").is_err()); // 盘符相对路径非绝对
    }

    #[cfg(not(windows))]
    #[test]
    fn require_absolute_unix_forms() {
        assert!(require_absolute("/a/b").is_ok());
        assert!(require_absolute("a/b").is_err());
        assert!(require_absolute("").is_err());
    }

    // ---- canonicalize_limited ----

    #[test]
    fn canonicalize_resolves_dotdot_and_existence() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a");
        std::fs::create_dir(&dir).unwrap();
        let f = dir.join("f.txt");
        std::fs::write(&f, "x").unwrap();

        let p = dir.join("..").join("a").join("f.txt");
        let c = canonicalize_limited(&p).unwrap();
        assert!(c.ends_with("f.txt"));
        let base = canonicalize_limited(tmp.path()).unwrap();
        assert!(is_within(&base, &c));

        assert!(canonicalize_limited(&dir.join("nope")).is_err());
        assert!(canonicalize_limited(Path::new("rel/path")).is_err());
    }

    fn make_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link)
        }
    }

    #[test]
    fn canonicalize_limits_link_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        let l1 = tmp.path().join("l1");
        let lself = tmp.path().join("lself");
        // Windows 需符号链接权限（管理员/开发者模式）；无权限时本用例跳过。
        if make_dir_link(&a, &l1).is_err() || make_dir_link(&lself, &lself).is_err() {
            eprintln!("skip canonicalize_limits_link_depth: symlink privilege unavailable");
            return;
        }
        // 深度 1：正常解析
        assert!(canonicalize_limited(&l1).is_ok());
        // 自环：深度超限
        assert!(canonicalize_limited(&lself).is_err());
    }
}
