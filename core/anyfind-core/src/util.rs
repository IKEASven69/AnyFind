//! 通用工具：时间格式、项目名推断、UUID 提取、snippet 哨兵高亮。

/// epoch 毫秒 → ISO 字符串（本地时区）；非法输入返回 ''。
pub fn iso_ms(ms: f64) -> String {
    if !ms.is_finite() {
        return String::new();
    }
    let secs = (ms / 1000.0) as i64;
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// 文件 mtime（秒）→ ISO；None/非法 → None。
pub fn iso_ts(ts: Option<f64>) -> Option<String> {
    let ts = ts?;
    if !ts.is_finite() || ts <= 0.0 {
        return None;
    }
    chrono::DateTime::from_timestamp(ts as i64, 0).map(|t| {
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    })
}

/// cwd → 项目名（末段路径）；空 → None。
pub fn project_from_cwd(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .last()
        .map(|s| s.to_string())
}

/// 从字符串里提取第一个 UUID（大小写不敏感）。
pub fn extract_uuid(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let n = b.len();
    if n < 36 {
        return None;
    }
    for i in 0..=(n - 36) {
        let w = &b[i..i + 36];
        if w[8] == b'-'
            && w[13] == b'-'
            && w[18] == b'-'
            && w[23] == b'-'
            && w.iter().enumerate().all(|(j, c)| {
                matches!(j, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit()
            })
        {
            return Some(s[i..i + 36].to_string());
        }
    }
    None
}

// ---- FTS5 snippet 高亮哨兵（私用区字符，原文几乎不可能出现）----
// 统一先 HTML 转义再放行哨兵 → <mark>，防 XSS 与双层包裹。

pub const SNIP_OPEN: char = '\u{E000}';
pub const SNIP_CLOSE: char = '\u{E001}';

/// FTS snippet 净化：先对原文 HTML 转义（& < >），再把哨兵转成 <mark>。
pub fn render_snippet(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if c == SNIP_OPEN => out.push_str("<mark>"),
            c if c == SNIP_CLOSE => out.push_str("</mark>"),
            c => out.push(c),
        }
    }
    out
}

/// 本地代码自行定位命中位置生成 snippet（LIKE 兜底路径用）。
/// 在 text 中找第一个 term 的命中点，截取前后各 half 个字符，命中处包哨兵。
pub fn make_snippet(text: &str, terms: &[String], half: usize) -> String {
    let lower = text.to_lowercase();
    let mut best: Option<usize> = None;
    let mut best_len = 0usize;
    for t in terms {
        let tl = t.to_lowercase();
        if tl.is_empty() {
            continue;
        }
        if let Some(pos) = lower.find(&tl) {
            if best.is_none() || pos < best.unwrap() {
                best = Some(pos);
                best_len = tl.len();
            }
        }
    }
    let (pos, tlen) = match (best, best_len) {
        (Some(p), l) => (p, l),
        _ => {
            // 无命中：取开头 half*2 字符
            let end = char_boundary(text, half * 2);
            return text[..end].to_string();
        }
    };
    let start = char_boundary_rev(text, pos.saturating_sub(half));
    let end = char_boundary(text, (pos + tlen + half).min(text.len()));
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&text[start..pos]);
    out.push(SNIP_OPEN);
    out.push_str(&text[pos..(pos + tlen).min(text.len())]);
    out.push(SNIP_CLOSE);
    out.push_str(&text[(pos + tlen).min(text.len())..end]);
    if end < text.len() {
        out.push('…');
    }
    out
}

fn char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn char_boundary_rev(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_extract() {
        assert_eq!(
            extract_uuid("rollout-2026-10-01T12-00-00-9f8e7d6c-1a2b-3c4d-8e9f-0a1b2c3d4e5f.jsonl"),
            Some("9f8e7d6c-1a2b-3c4d-8e9f-0a1b2c3d4e5f".to_string())
        );
        assert_eq!(extract_uuid("no-uuid-here"), None);
    }

    #[test]
    fn snippet_escape() {
        let raw = format!("a <b> & {SNIP_OPEN}x{SNIP_CLOSE}");
        assert_eq!(render_snippet(&raw), "a &lt;b&gt; &amp; <mark>x</mark>");
    }

    #[test]
    fn project_name() {
        assert_eq!(project_from_cwd("D:\\coding\\anyfind"), Some("anyfind".into()));
        assert_eq!(project_from_cwd("D:/coding/anyfind/"), Some("anyfind".into()));
        assert_eq!(project_from_cwd(""), None);
    }
}
