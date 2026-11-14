//! file_ops：/api/file/* 端点的安全纯函数（docs/api-contract.md v0.2）。
//!
//! 覆盖可单测逻辑：Windows 保留名/非法字符/末尾点空格/长度 校验、噪声过滤、
//! 行分页、op 请求整形（paths/target/newName）、containment（目标不得越出
//! 目录、不得位于源目录内）、符号链接深度 ≤ MAX_LINK_DEPTH。
//! 路径解析 canonicalize_limited 逐组件解析（`.`/`..` 词法归一 + 跟随链接），
//! 最后 std::fs::canonicalize 归一（大小写 / 8.3 / verbatim）。

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// 符号链接最大解析深度（超过即拒绝）。
pub const MAX_LINK_DEPTH: u32 = 32;

/// 默认隐藏的噪声目录（大小写不敏感精确匹配，仅目录）。
pub const NOISE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "dist",
    "build",
    "coverage",
    ".idea",
    "target",
];

pub const LIST_MAX_DEFAULT: usize = 500;
pub const LIST_MAX_LIMIT: usize = 2000;
pub const READ_LINES_DEFAULT: usize = 30;
pub const READ_LINES_MAX: usize = 500;
/// read 大小上限：2MiB（严格大于 → 413）。
pub const READ_SIZE_MAX: u64 = 2 * 1024 * 1024;
/// read 二进制嗅探窗口：前 8KiB 含 NUL 即视为二进制。
pub const READ_SNIFF_BYTES: usize = 8 * 1024;
/// /api/file/op 单请求 paths 上限（超过 → 400）。
pub const OP_PATHS_MAX: usize = 128;

/// Windows 保留设备名（按第一个点前的主干判断，大小写不敏感）。
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// op 单条执行项：path=请求原路径（results 回显），src/dest=执行用（"" 表示无）。
#[derive(Clone, Debug, PartialEq)]
pub struct OpItem {
    pub path: String,
    pub src: String,
    pub dest: String,
}

// ---------- 名称校验 ----------

/// 新建/重命名目标名校验：非空、非 "."/".."、无控制与非法字符
/// （`<>:"/\|?*`）、不以点/空格结尾、非 Windows 保留名、长度 ≤255
/// （按 UTF-16 码元计，与 NTFS 单组件上限一致）。
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    if name.encode_utf16().count() > 255 {
        return Err(format!(
            "名称过长（上限 255）: {}…",
            name.chars().take(32).collect::<String>()
        ));
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

/// 目标路径 = 目录 + 单段名校验后的名称；做前缀 containment（防 `../` 逃逸）。
pub fn contained_dest(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let dest = dir.join(name);
    if !is_within(dir, &dest) {
        return Err(format!("目标越出目录: {}", dest.display()));
    }
    Ok(dest)
}

// ---------- 噪声 / 参数钳制 ----------

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

// ---------- 二进制嗅探 ----------

/// 前 8KiB（或全文件）含 NUL → 二进制。
pub fn is_binary_prefix(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(READ_SNIFF_BYTES)].contains(&0)
}

// ---------- base64 编码（file/read?format=base64 的取字节通路） ----------

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码（含 `=` 填充）。手写以避免为 ~20 行逻辑引入外部 crate（供应链纪律）。
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(B64_ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64_ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------- 行分页 ----------

/// 行分页结果。
pub struct Page {
    pub lines: Vec<String>,
    pub total: usize,
    pub truncated: bool,
}

/// 按行分页：`\n` 切分、去行尾 `\r`、文件尾随换行不产生空行；
/// 从 `offset` 起取 `lines` 行。`truncated` = 窗口已触及/超过文件末尾
/// （返回行数可能少于请求行数；与桌面 UI 契约实现一致）。
pub fn paginate(content: &str, offset: usize, lines: usize) -> Page {
    let all = split_lines(content);
    let total = all.len();
    let start = offset.min(total);
    let end = offset.saturating_add(lines).min(total);
    let got = &all[start..end];
    Page {
        truncated: offset.saturating_add(got.len()) >= total,
        lines: got.iter().map(|s| (*s).to_string()).collect(),
        total,
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

// ---------- 目录条目准备 ----------

/// 目录条目（list 用）。
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<String>,
}

/// 噪声过滤（仅隐藏目录）+ 目录优先/同组按名排序（大小写不敏感，
/// 同名再按原名）+ 截断。返回 (条目, truncated)。
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

// ---------- 路径解析与 containment ----------

/// 路径必须非空且绝对（Windows 盘符/UNC 由 Path 判定，反斜杠原生支持）。
pub fn require_absolute(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("路径不能为空".into());
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(format!("需要绝对路径: {raw}"))
    }
}

/// 前缀 containment（组件级比较，Windows 大小写不敏感，去 verbatim 前缀）。
/// `D:\a` 不包含 `D:\ab`（组件边界，非字符串前缀）。
pub fn is_within(base: &Path, child: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| {
                let s = c.as_os_str().to_string_lossy().to_lowercase();
                if let Some(rest) = s.strip_prefix("\\\\?\\unc\\") {
                    format!("\\\\{rest}")
                } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
                    rest.to_string()
                } else {
                    s
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

/// 删除/回收前的根判定（比 `is_root_path` 更强，防绕过）：
/// 词法归一（无 fs 访问——`D:\不存在\..` 按 Win32 `-LiteralPath` 的词法解析
/// 同样是根，中间组件不存在不能跳过判定）**或** canonicalize（跟符号链接到根）
/// 任一为根 → true。
pub fn is_root_path_resolved(p: &Path) -> bool {
    is_root_path(&lexically_resolve(p))
        || canonicalize_limited(p)
            .map(|c| is_root_path(&c))
            .unwrap_or(false)
}

/// 词法归一（无 fs 访问）：消解 `.` 与 `..`（不越过根）。用于 plan 阶段
/// 在 canonicalize 之前对含 `..` 的路径做 containment 判定。
fn lexically_resolve(p: &Path) -> PathBuf {
    let (root, rest) = split_root(p);
    let mut out = root;
    for c in rest {
        if c == ".." {
            if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                out.pop();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 词法归一后按组件比较两目录是否同一目录（Windows 大小写不敏感，
/// `.`,`..` 已消解、尾部分隔符无影响）。纯函数不做 fs 访问——
/// 符号链接归一由调用方对 target 先行 canonicalize 后再进 plan。
pub fn same_lexical_dir(a: &Path, b: &Path) -> bool {
    let la = lexically_resolve(a);
    let lb = lexically_resolve(b);
    is_within(&la, &lb) && is_within(&lb, &la)
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
            if matches!(
                resolved.components().next_back(),
                Some(Component::Normal(_))
            ) {
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
    std::fs::canonicalize(&resolved).map_err(|e| format!("路径不可读 {}: {e}", resolved.display()))
}

/// GET 端点路径解析：绝对路径 + canonicalize（`.`/`..` 与链接归一到真实路径）。
pub fn resolve_request_path(raw: &str) -> Result<PathBuf, String> {
    let p = require_absolute(raw)?;
    canonicalize_limited(&p)
}

// ---------- op 请求整形（纯） ----------

/// 校验 op 请求结构并计算每条执行项（src/dest）。
/// 结构错误 → Err（调用方映射 400）；文件系统状态（存在性等）不在此检查。
pub fn plan_op(
    op: &str,
    paths: &[String],
    target: Option<&str>,
    new_name: Option<&str>,
) -> Result<Vec<OpItem>, String> {
    if !matches!(
        op,
        "rename" | "copy" | "cut" | "recycle" | "newFile" | "newDir"
    ) {
        return Err(format!("不支持的 op: {op}"));
    }
    let single = matches!(op, "rename" | "newFile" | "newDir");
    if paths.is_empty() {
        return Err("paths 不能为空".into());
    }
    if paths.len() > OP_PATHS_MAX {
        return Err(format!("paths 超过上限 {OP_PATHS_MAX}"));
    }
    if single && paths.len() != 1 {
        return Err(format!("{op} 需要恰好 1 条 path"));
    }
    for p in paths {
        require_absolute(p)?;
    }
    if matches!(op, "rename" | "copy" | "cut") {
        let t = target.ok_or("缺少 target")?;
        require_absolute(t)?;
    }
    if single {
        let n = new_name.ok_or("缺少 newName")?;
        validate_name(n)?;
    }
    match op {
        "rename" => {
            let target = Path::new(target.unwrap());
            let src = Path::new(&paths[0]);
            // 契约：rename 不跨目录，target 必须就是 src 所在目录。
            // 词法归一（`.`/`..` 消解）后按组件比较（Windows 大小写不敏感），
            // 堵住 `target=D:\dir\..\..` 借 canonicalize 越级把文件挪到任意位置。
            let parent = src
                .parent()
                .ok_or_else(|| format!("无法确定源所在目录: {}", paths[0]))?;
            if !same_lexical_dir(parent, target) {
                return Err(format!(
                    "rename 不跨目录：target 必须是源所在目录（src={}, target={}）",
                    paths[0],
                    target.display()
                ));
            }
            let dest = contained_dest(target, new_name.unwrap())?;
            Ok(vec![OpItem {
                path: paths[0].clone(),
                src: paths[0].clone(),
                dest: dest.to_string_lossy().into_owned(),
            }])
        }
        "copy" | "cut" => {
            let t = Path::new(target.unwrap());
            let mut out = Vec::with_capacity(paths.len());
            for p in paths {
                // 防自嵌套：target 位于源路径子树内（含相等）时，Copy-Item/Move-Item -Recurse
                // 会沿不断增长的子树无限递归复制直至超时（终审实测 copy X → X/sub 触发，
                // 磁盘残留多层嵌套副本）——在计划层直接拒绝。
                if is_within(Path::new(p), t) {
                    return Err(format!(
                        "目标目录位于源目录子树内（{p} → {}），已拒绝以避免递归复制",
                        t.display()
                    ));
                }
                let leaf = Path::new(p)
                    .file_name()
                    .ok_or_else(|| format!("无法确定源文件名: {p}"))?;
                let dest = t.join(leaf);
                if !is_within(t, &dest) {
                    return Err(format!("目标越出 target: {}", dest.display()));
                }
                out.push(OpItem {
                    path: p.clone(),
                    src: p.clone(),
                    dest: dest.to_string_lossy().into_owned(),
                });
            }
            Ok(out)
        }
        "recycle" => Ok(paths
            .iter()
            .map(|p| OpItem {
                path: p.clone(),
                src: p.clone(),
                dest: String::new(),
            })
            .collect()),
        _ => {
            // newFile / newDir：paths[0] = 父目录
            let dest = contained_dest(Path::new(&paths[0]), new_name.unwrap())?;
            Ok(vec![OpItem {
                path: paths[0].clone(),
                src: String::new(),
                dest: dest.to_string_lossy().into_owned(),
            }])
        }
    }
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

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // ---- b64_encode（对照 Node Buffer.toString('base64') 已知向量） ----

    #[test]
    fn b64_known_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        // PDF 魔数头（取字节通路的首要用例；与 data:application/pdf;base64,JVBERi0 前缀一致）。
        assert_eq!(b64_encode(b"%PDF-1.7\n"), "JVBERi0xLjcK");
    }

    #[test]
    fn b64_all_byte_values_roundtrip_shape() {
        let all: Vec<u8> = (0..=255u8).collect();
        let enc = b64_encode(&all);
        assert_eq!(enc.len(), (256 + 2) / 3 * 4);
        // 仅标准字母表字符 + 填充。
        assert!(enc.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='));
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
            "CON.txt", "nul.log", "a<b", "a>b", "a:b", "a\"b", "a/b", "a\\b", "a|b", "a?b", "a*b",
            "x\u{0}y",
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

    #[test]
    fn contained_dest_joins_and_blocks_escape() {
        #[cfg(windows)]
        let base = Path::new(r"D:\target");
        #[cfg(not(windows))]
        let base = Path::new("/target");
        let d = contained_dest(base, "ok.txt").unwrap();
        assert!(is_within(base, &d));
        assert!(contained_dest(base, "..").is_err());
        assert!(contained_dest(base, "a/b").is_err());
        assert!(contained_dest(base, "a\\b").is_err());
        assert!(contained_dest(base, "CON").is_err());
    }

    // ---- 噪声 / 参数钳制 ----

    #[test]
    fn noise_matching_is_case_insensitive_exact() {
        for n in [
            ".git",
            "NODE_MODULES",
            "Dist",
            "build",
            "coverage",
            ".IDEA",
            "target",
        ] {
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
        assert!(is_binary_prefix(b"ab\0cd"));
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
        assert!(!p.truncated); // 窗口未到末尾

        let p = paginate("a\nb\nc", 2, 10);
        assert_eq!(p.lines, vec!["c"]);
        assert!(p.truncated); // 已到文件末尾

        let p = paginate("a\nb\nc", 1, 1);
        assert_eq!(p.lines, vec!["b"]);
        // 窗口未到末尾（2 < 3）→ truncated=false；与桌面 UI mock 的
        // `off + ln >= total` 语义一致：truncated=true 表示已到/超过文件末尾。
        assert!(!p.truncated);

        let p = paginate("a\r\nb", 0, 10);
        assert_eq!(p.lines, vec!["a", "b"]);
        assert_eq!(p.total, 2);
    }

    #[test]
    fn paginate_edge_cases() {
        let p = paginate("", 0, 10);
        assert!(p.lines.is_empty());
        assert_eq!(p.total, 0);
        assert!(p.truncated); // 空文件视作已到末尾（与 UI 契约一致）

        let p = paginate("a\nb\n", 0, 10); // 尾随换行不产生空行
        assert_eq!(p.lines, vec!["a", "b"]);
        assert_eq!(p.total, 2);
        assert!(p.truncated);

        let p = paginate("\n", 0, 10); // 单换行 = 一个空行
        assert_eq!(p.lines, vec![""]);
        assert_eq!(p.total, 1);

        let p = paginate("a\nb\nc", 99, 10); // offset 越界
        assert!(p.lines.is_empty());
        assert!(p.truncated);

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
        assert!(!is_within(Path::new(r"D:\a"), Path::new(r"D:\ab"))); // 组件边界
        assert!(!is_within(Path::new(r"D:\a\b"), Path::new(r"D:\a")));
        assert!(is_within(Path::new(r"D:\A"), Path::new(r"d:\a\b"))); // 大小写不敏感
        assert!(is_within(Path::new(r"\\?\D:\a"), Path::new(r"D:\a\b"))); // verbatim 归一
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

    // ---- canonicalize / resolve ----

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
        assert!(canonicalize_limited(&dir.join("nope")).is_err());
        assert!(canonicalize_limited(Path::new("rel/path")).is_err());

        // resolve_request_path：绝对存在 → Ok；缺失/相对 → Err
        assert!(resolve_request_path(&dir.to_string_lossy()).is_ok());
        assert!(resolve_request_path(&dir.join("nope").to_string_lossy()).is_err());
        assert!(resolve_request_path("rel/path").is_err());
        assert!(resolve_request_path("").is_err());
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
        assert!(canonicalize_limited(&l1).is_ok()); // 深度 1 正常
        assert!(canonicalize_limited(&lself).is_err()); // 自环 → 深度超限
    }

    // ---- plan_op ----

    #[cfg(windows)]
    mod plan {
        use super::*;

        #[test]
        fn plan_rename_same_dir() {
            let items = plan_op(
                "rename",
                &s(&[r"D:\a\old.txt"]),
                Some(r"D:\a"),
                Some("new.txt"),
            )
            .unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].path, r"D:\a\old.txt");
            assert_eq!(items[0].src, r"D:\a\old.txt");
            assert!(items[0].dest.ends_with(r"\new.txt"));
            assert!(items[0].dest.starts_with(r"D:\a"));
        }

        #[test]
        fn plan_rename_rejects_cross_dir_and_dotdot_escape() {
            // target 不是 src 所在目录 → 拒绝（契约：rename 不跨目录）
            assert!(plan_op(
                "rename",
                &s(&[r"D:\a\old.txt"]),
                Some(r"D:\b"),
                Some("new.txt")
            )
            .is_err());
            // target 越级（词法归一后指向别处，等于任意位置移动）→ 拒绝
            assert!(plan_op(
                "rename",
                &s(&[r"D:\a\old.txt"]),
                Some(r"D:\a\..\.."),
                Some("new.txt")
            )
            .is_err());
            assert!(plan_op(
                "rename",
                &s(&[r"D:\a\old.txt"]),
                Some(r"D:\a\sub\..\.."),
                Some("new.txt")
            )
            .is_err());
            // 尾部斜杠 / 大小写差异 / `..` 抵消 → 仍视为同目录，放行
            assert!(plan_op(
                "rename",
                &s(&[r"D:\a\old.txt"]),
                Some(r"D:\a\"),
                Some("new.txt")
            )
            .is_ok());
            assert!(plan_op(
                "rename",
                &s(&[r"d:\A\old.txt"]),
                Some(r"D:\a"),
                Some("new.txt")
            )
            .is_ok());
            assert!(plan_op(
                "rename",
                &s(&[r"D:\a\sub\..\old.txt"]),
                Some(r"D:\a"),
                Some("new.txt")
            )
            .is_ok());
            // 盘符根下的文件（parent = D:\）合法
            assert!(plan_op(
                "rename",
                &s(&[r"D:\old.txt"]),
                Some(r"D:\"),
                Some("new.txt")
            )
            .is_ok());
        }

        #[test]
        fn plan_copy_cut_build_dest_from_leaf() {
            let items = plan_op(
                "copy",
                &s(&[r"D:\a\x.txt", r"D:\b\y.txt"]),
                Some(r"D:\t"),
                None,
            )
            .unwrap();
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].dest, r"D:\t\x.txt");
            assert_eq!(items[1].dest, r"D:\t\y.txt");

            let items = plan_op("cut", &s(&[r"D:\a\x.txt"]), Some(r"D:/t/"), None).unwrap();
            assert!(items[0].dest.starts_with(r"D:") && items[0].dest.ends_with("x.txt"));
        }

        #[test]
        fn plan_copy_cut_rejects_target_inside_source_subtree() {
            // 终审实测：copy D:\selfcopy → D:\selfcopy\sub 触发 Copy-Item -Recurse 无限自嵌套复制。
            let err = plan_op(
                "copy",
                &s(&[r"D:\selfcopy"]),
                Some(r"D:\selfcopy\sub"),
                None,
            )
            .unwrap_err();
            assert!(err.contains("源目录子树"), "实际错误：{err}");

            // cut 同样拒绝；target == 源本身也拒绝（含相等）。
            assert!(plan_op("cut", &s(&[r"D:\x"]), Some(r"D:\x"), None).is_err());
            // 大小写与分隔符不敏感路径（is_within 归一化语义）同样命中。
            assert!(plan_op("copy", &s(&[r"D:\X"]), Some(r"d:/x/sub"), None).is_err());
            // 正常兄弟目录不受影响。
            assert!(plan_op("copy", &s(&[r"D:\a"]), Some(r"D:\b"), None).is_ok());
        }

        #[test]
        fn plan_recycle_keeps_paths() {
            let items = plan_op("recycle", &s(&[r"D:\a", r"D:\b"]), None, None).unwrap();
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].src, r"D:\a");
            assert_eq!(items[0].dest, "");
        }

        #[test]
        fn plan_new_file_and_dir() {
            let items = plan_op("newFile", &s(&[r"D:\dir"]), None, Some("n.txt")).unwrap();
            assert_eq!(items[0].src, "");
            assert!(items[0].dest.ends_with(r"\n.txt"));

            let items = plan_op("newDir", &s(&[r"D:\dir"]), None, Some("sub")).unwrap();
            assert!(items[0].dest.ends_with(r"\sub"));
        }

        #[test]
        fn plan_rejects_bad_structure() {
            // 未知 op
            assert!(plan_op("delete", &s(&[r"D:\a"]), None, None).is_err());
            // 空 / 超限 paths
            assert!(plan_op("recycle", &[], None, None).is_err());
            let many: Vec<String> = (0..=OP_PATHS_MAX).map(|i| format!(r"D:\f{i}")).collect();
            assert!(plan_op("recycle", &many, None, None).is_err());
            // 单路径 op 传多条
            assert!(plan_op("rename", &s(&[r"D:\a", r"D:\b"]), Some(r"D:\a"), Some("x")).is_err());
            assert!(plan_op("newFile", &s(&[r"D:\a", r"D:\b"]), None, Some("x")).is_err());
            // 非绝对 path / target
            assert!(plan_op("recycle", &s(&["rel"]), None, None).is_err());
            assert!(plan_op("copy", &s(&[r"D:\a"]), Some("rel"), None).is_err());
            // 缺 target / newName
            assert!(plan_op("rename", &s(&[r"D:\a"]), None, Some("x")).is_err());
            assert!(plan_op("copy", &s(&[r"D:\a"]), None, None).is_err());
            assert!(plan_op("rename", &s(&[r"D:\a"]), Some(r"D:\a"), None).is_err());
            assert!(plan_op("newFile", &s(&[r"D:\a"]), None, None).is_err());
            // 非法 newName
            assert!(plan_op("rename", &s(&[r"D:\a"]), Some(r"D:\a"), Some("CON")).is_err());
            assert!(plan_op("newDir", &s(&[r"D:\a"]), None, Some("x/y")).is_err());
            assert!(plan_op("newFile", &s(&[r"D:\a"]), None, Some("trailing.")).is_err());
            // copy 源无文件名（盘符根）
            assert!(plan_op("copy", &s(&[r"D:\"]), Some(r"D:\t"), None).is_err());
        }
    }
}
