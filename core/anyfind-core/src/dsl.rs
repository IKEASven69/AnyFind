//! 查询 DSL v1 解析器（SPEC：做深不做大）。
//!
//! 语法：
//!   裸词 AND；`"短语"` 精确；`a OR b`；`-排除`
//!   字段：kind:file|content|session|skill|memory
//!         agent:claude-code|codex|dsh|opencode|pi|cursor
//!         project:X   ext:pdf;docx   size:>10mb|1mb..5mb
//!         date:today|week|>2026-10-01|2026-10-01..2026-10-20
//!         in:<路径前缀>   role:user|assistant（session 专用）
//! 非法语法不吞掉：进 errors 带 span，UI 画红波浪。

#[derive(Debug, Clone, PartialEq)]
pub enum TermExpr {
    And(Vec<TermExpr>),
    Or(Vec<TermExpr>),
    Not(Box<TermExpr>),
    Word(String),
    Phrase(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SizeRange {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateRange {
    /// epoch 秒（本地时区）
    pub from: Option<f64>,
    pub to: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Kind(Vec<String>),
    Agent(Vec<String>),
    Project(String),
    Ext(Vec<String>),
    Size(SizeRange),
    Date(DateRange),
    In(String),
    Role(String),
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub expr: Option<TermExpr>,
    pub filters: Vec<Filter>,
    pub errors: Vec<ParseError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub span: (usize, usize),
    pub message: String,
}

const FIELD_KEYS: &[&str] = &["kind", "agent", "project", "ext", "size", "date", "in", "role"];

// ---------- 分词 ----------

#[derive(Debug, Clone, PartialEq)]
struct Token {
    text: String,
    span: (usize, usize),
    quoted: bool,
    negated: bool,
}

fn tokenize(input: &str) -> (Vec<Token>, Vec<ParseError>) {
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let (start, c) = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let mut negated = false;
        let mut j = i;
        let mut real_start = start;
        if c == '-' {
            negated = true;
            j += 1;
            if j >= n {
                errors.push(ParseError {
                    span: (start, start + 1),
                    message: "单独的 '-' 后需要跟排除词".into(),
                });
                break;
            }
            real_start = chars[j].0;
        }
        if chars[j].1 == '"' {
            // 短语
            let qstart = chars[j].0;
            j += 1;
            let mut text = String::new();
            let mut closed = false;
            while j < n {
                let (_, c2) = chars[j];
                if c2 == '"' {
                    closed = true;
                    j += 1;
                    break;
                }
                text.push(c2);
                j += 1;
            }
            let end = if j < n { chars[j].0 } else { input.len() };
            if !closed {
                errors.push(ParseError {
                    span: (qstart, end),
                    message: "引号未闭合".into(),
                });
            }
            tokens.push(Token { text, span: (start, end), quoted: true, negated });
            i = j;
        } else {
            let mut text = String::new();
            while j < n && !chars[j].1.is_whitespace() {
                text.push(chars[j].1);
                j += 1;
            }
            let end = if j < n { chars[j].0 } else { input.len() };
            let _ = real_start;
            tokens.push(Token { text, span: (start, end), quoted: false, negated });
            i = j;
        }
    }
    (tokens, errors)
}

// ---------- 字段值解析 ----------

fn parse_size_val(s: &str) -> Option<u64> {
    let lower = s.to_lowercase();
    let (num_part, mult) = if let Some(p) = lower.strip_suffix("kb") {
        (p, 1u64 << 10)
    } else if let Some(p) = lower.strip_suffix("mb") {
        (p, 1u64 << 20)
    } else if let Some(p) = lower.strip_suffix("gb") {
        (p, 1u64 << 30)
    } else if let Some(p) = lower.strip_suffix('b') {
        (p, 1)
    } else {
        (lower.as_str(), 1)
    };
    let v: f64 = num_part.parse().ok()?;
    if v < 0.0 {
        return None;
    }
    Some((v * mult as f64) as u64)
}

fn parse_size(s: &str) -> Option<SizeRange> {
    if let Some(rest) = s.strip_prefix('>') {
        return Some(SizeRange { min: Some(parse_size_val(rest)?), max: None });
    }
    if let Some(rest) = s.strip_prefix('<') {
        return Some(SizeRange { min: None, max: Some(parse_size_val(rest)?) });
    }
    if let Some((a, b)) = s.split_once("..") {
        let min = if a.is_empty() { None } else { Some(parse_size_val(a)?) };
        let max = if b.is_empty() { None } else { Some(parse_size_val(b)?) };
        return Some(SizeRange { min, max });
    }
    let v = parse_size_val(s)?;
    Some(SizeRange { min: Some(v), max: Some(v) })
}

fn parse_date_one(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn day_start_ts(d: chrono::NaiveDate) -> Option<f64> {
    d.and_hms_opt(0, 0, 0)?
        .and_local_timezone(chrono::Local)
        .single()
        .map(|t| t.timestamp() as f64)
}

fn day_end_ts(d: chrono::NaiveDate) -> Option<f64> {
    d.and_hms_opt(23, 59, 59)?
        .and_local_timezone(chrono::Local)
        .single()
        .map(|t| t.timestamp() as f64)
}

fn parse_date(s: &str) -> Option<DateRange> {
    let today = chrono::Local::now().date_naive();
    match s {
        "today" => {
            return Some(DateRange { from: day_start_ts(today), to: day_end_ts(today) });
        }
        "week" => {
            let from = today - chrono::Duration::days(6);
            return Some(DateRange { from: day_start_ts(from), to: day_end_ts(today) });
        }
        _ => {}
    }
    if let Some(rest) = s.strip_prefix('>') {
        let d = parse_date_one(rest)?;
        return Some(DateRange { from: day_start_ts(d), to: None });
    }
    if let Some(rest) = s.strip_prefix('<') {
        let d = parse_date_one(rest)?;
        return Some(DateRange { from: None, to: day_end_ts(d) });
    }
    if let Some((a, b)) = s.split_once("..") {
        let da = parse_date_one(a)?;
        let db = parse_date_one(b)?;
        return Some(DateRange { from: day_start_ts(da), to: day_end_ts(db) });
    }
    let d = parse_date_one(s)?;
    Some(DateRange { from: day_start_ts(d), to: day_end_ts(d) })
}

fn parse_filter(key: &str, value: &str, span: (usize, usize)) -> Result<Filter, ParseError> {
    let bad = |msg: &str| ParseError { span, message: format!("{}:{} — {}", key, value, msg) };
    match key {
        "kind" => {
            let vals: Vec<String> =
                value.split(';').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
            const VALID: &[&str] = &["file", "content", "session", "skill", "memory"];
            if vals.is_empty() || !vals.iter().all(|v| VALID.contains(&v.as_str())) {
                return Err(bad("kind 只能是 file|content|session|skill|memory（;分隔）"));
            }
            Ok(Filter::Kind(vals))
        }
        "agent" => {
            let vals: Vec<String> =
                value.split(';').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
            const VALID: &[&str] =
                &["claude-code", "codex", "dsh", "opencode", "pi", "cursor"];
            if vals.is_empty() || !vals.iter().all(|v| VALID.contains(&v.as_str())) {
                return Err(bad("agent 只能是 claude-code|codex|dsh|opencode|pi|cursor"));
            }
            Ok(Filter::Agent(vals))
        }
        "project" => {
            if value.is_empty() {
                return Err(bad("project 值不能为空"));
            }
            Ok(Filter::Project(value.to_string()))
        }
        "ext" => {
            let vals: Vec<String> = value
                .split(';')
                .map(|s| s.trim().trim_start_matches('.').to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            if vals.is_empty() {
                return Err(bad("ext 需要至少一个扩展名（;分隔）"));
            }
            Ok(Filter::Ext(vals))
        }
        "size" => parse_size(value).map(Filter::Size).ok_or_else(|| bad("size 语法: >10mb | <5mb | 1mb..5mb")),
        "date" => parse_date(value)
            .map(Filter::Date)
            .ok_or_else(|| bad("date 语法: today | week | >2026-10-01 | 2026-10-01..2026-10-20")),
        "in" => {
            if value.is_empty() {
                return Err(bad("in 需要路径前缀"));
            }
            Ok(Filter::In(value.to_string()))
        }
        "role" => {
            let v = value.to_lowercase();
            if v != "user" && v != "assistant" {
                return Err(bad("role 只能是 user|assistant"));
            }
            Ok(Filter::Role(v))
        }
        _ => Err(bad("未知字段")),
    }
}

// ---------- 主入口 ----------

pub fn parse(input: &str) -> Query {
    let mut q = Query::default();
    let (tokens, errors) = tokenize(input);
    q.errors.extend(errors);

    // 顶层 OR 分组：token 序列按裸 "OR" 切开，组内 AND。
    // 否定项（-x）是全局排除，不进 OR 分支（`a OR b -c` = (a OR b) AND NOT c）。
    let mut or_groups: Vec<Vec<TermExpr>> = vec![Vec::new()];
    let mut excludes: Vec<TermExpr> = Vec::new();

    for tok in &tokens {
        if !tok.quoted && !tok.negated && tok.text == "OR" {
            if or_groups.last().is_some_and(|g| g.is_empty()) {
                q.errors.push(ParseError {
                    span: tok.span,
                    message: "OR 前缺少操作数".into(),
                });
            } else {
                or_groups.push(Vec::new());
            }
            continue;
        }
        // 字段 token？
        if !tok.quoted && !tok.negated {
            if let Some((key, value)) = tok.text.split_once(':') {
                if FIELD_KEYS.contains(&key) && !value.is_empty() {
                    match parse_filter(key, value, tok.span) {
                        Ok(f) => q.filters.push(f),
                        Err(e) => q.errors.push(e),
                    }
                    continue;
                }
            }
        }
        let term = if tok.quoted { TermExpr::Phrase(tok.text.clone()) } else { TermExpr::Word(tok.text.clone()) };
        if tok.negated {
            excludes.push(term);
        } else {
            or_groups.last_mut().unwrap().push(term);
        }
    }

    let groups: Vec<TermExpr> = or_groups
        .into_iter()
        .filter(|g| !g.is_empty())
        .map(|g| if g.len() == 1 { g.into_iter().next().unwrap() } else { TermExpr::And(g) })
        .collect();
    let positive = match groups.len() {
        0 => None,
        1 => Some(groups.into_iter().next().unwrap()),
        _ => Some(TermExpr::Or(groups)),
    };
    let mut top: Vec<TermExpr> = Vec::new();
    if let Some(p) = positive {
        top.push(p);
    }
    top.extend(excludes.into_iter().map(|t| TermExpr::Not(Box::new(t))));
    q.expr = match top.len() {
        0 => None,
        1 => Some(top.into_iter().next().unwrap()),
        _ => Some(TermExpr::And(top)),
    };
    q
}

// ---------- 查询执行辅助 ----------

/// 收集全部肯定文本项（Word/Phrase，非 Not 内）用于 LIKE 兜底。
pub fn positive_terms(q: &Query) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(e: &TermExpr, negated: bool, out: &mut Vec<String>) {
        match e {
            TermExpr::And(v) | TermExpr::Or(v) => v.iter().for_each(|x| walk(x, negated, out)),
            TermExpr::Not(x) => walk(x, true, out),
            TermExpr::Word(w) | TermExpr::Phrase(w) => {
                if !negated && !w.is_empty() {
                    out.push(w.clone());
                }
            }
        }
    }
    if let Some(e) = &q.expr {
        walk(e, false, &mut out);
    }
    out
}

/// 用 expr 在一段文本上做布尔匹配（LIKE 兜底路径，大小写不敏感）。
pub fn text_matches(expr: &TermExpr, haystack_lower: &str) -> bool {
    match expr {
        TermExpr::And(v) => v.iter().all(|e| text_matches(e, haystack_lower)),
        TermExpr::Or(v) => v.iter().any(|e| text_matches(e, haystack_lower)),
        TermExpr::Not(e) => !text_matches(e, haystack_lower),
        TermExpr::Word(w) | TermExpr::Phrase(w) => {
            haystack_lower.contains(&w.to_lowercase())
        }
    }
}

/// expr 是否含需要复核的结构（OR / NOT / 短词）。纯 AND 的 ≥3 字符项
/// FTS5 trigram（case_insensitive）已保证精确语义，可跳过逐行复核。
pub fn expr_needs_verify(e: &TermExpr) -> bool {
    match e {
        TermExpr::And(v) | TermExpr::Or(v) => v.iter().any(expr_needs_verify),
        TermExpr::Not(_) => true,
        TermExpr::Word(w) | TermExpr::Phrase(w) => w.chars().count() < 3,
    }
}

/// expr 是否含 OR / NOT（LIKE 路径只需为这两种结构复核；
/// 纯 AND 的词/短语由 LIKE 链本身保证语义，无需逐行复核）。
pub fn expr_has_or_not(e: &TermExpr) -> bool {
    match e {
        TermExpr::Or(_) | TermExpr::Not(_) => true,
        TermExpr::And(v) => v.iter().any(expr_has_or_not),
        TermExpr::Word(_) | TermExpr::Phrase(_) => false,
    }
}

/// expr 是否含 ASCII 字母项（决定复核时是否需要对 haystack 做小写化）。
pub fn expr_has_ascii(e: &TermExpr) -> bool {
    match e {
        TermExpr::And(v) | TermExpr::Or(v) => v.iter().any(expr_has_ascii),
        TermExpr::Not(x) => expr_has_ascii(x),
        TermExpr::Word(w) | TermExpr::Phrase(w) => w.bytes().any(|b| b.is_ascii_alphabetic()),
    }
}

/// 复核入口：raw 原文 + 按需小写化（调用方保证 haystack_lower 在 needs_lower 时给出）。
pub fn text_matches_raw(expr: &TermExpr, raw: &str) -> bool {
    if expr_has_ascii(expr) {
        text_matches(expr, &raw.to_lowercase())
    } else {
        // 无 ASCII 字母：contains 本身即正确（CJK 无大小写）
        fn m(e: &TermExpr, h: &str) -> bool {
            match e {
                TermExpr::And(v) => v.iter().all(|x| m(x, h)),
                TermExpr::Or(v) => v.iter().any(|x| m(x, h)),
                TermExpr::Not(x) => !m(x, h),
                TermExpr::Word(w) | TermExpr::Phrase(w) => h.contains(w.as_str()),
            }
        }
        m(expr, raw)
    }
}

/// 构造 FTS5 MATCH 表达式（trigram tokenizer：每项转短语即子串匹配）。
/// 全部项 < 3 字符时返回 None（调用方降级 LIKE 路径）。
pub fn fts5_query(q: &Query) -> Option<String> {
    let e = q.expr.as_ref()?;
    fn esc(s: &str) -> String {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
    fn build(e: &TermExpr, out: &mut String) -> bool {
        match e {
            TermExpr::And(v) => {
                let mut first = true;
                for x in v {
                    // FTS5 的 NOT 不能单独存在；跳过纯否定项（由调用方 LIKE 复检）
                    if matches!(x, TermExpr::Not(_)) {
                        continue;
                    }
                    if !first {
                        out.push(' ');
                    }
                    if !build(x, out) {
                        return false;
                    }
                    first = false;
                }
                true
            }
            TermExpr::Or(v) => {
                out.push('(');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(" OR ");
                    }
                    if !build(x, out) {
                        return false;
                    }
                }
                out.push(')');
                true
            }
            TermExpr::Not(_) => true, // 顶层已跳过
            TermExpr::Word(w) | TermExpr::Phrase(w) => {
                if w.chars().count() < 3 {
                    return false; // trigram 无法匹配 <3 字符
                }
                out.push_str(&esc(w));
                true
            }
        }
    }
    let mut s = String::new();
    if build(e, &mut s) && !s.trim().is_empty() {
        Some(s)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_err(q: &Query) {
        assert!(q.errors.is_empty(), "unexpected errors: {:?}", q.errors);
    }

    #[test]
    fn bare_words_and() {
        let q = parse("nginx proxy");
        no_err(&q);
        assert_eq!(
            q.expr,
            Some(TermExpr::And(vec![
                TermExpr::Word("nginx".into()),
                TermExpr::Word("proxy".into())
            ]))
        );
    }

    #[test]
    fn phrase() {
        let q = parse("\"hello world\" foo");
        no_err(&q);
        assert_eq!(
            q.expr,
            Some(TermExpr::And(vec![
                TermExpr::Phrase("hello world".into()),
                TermExpr::Word("foo".into())
            ]))
        );
    }

    #[test]
    fn or_groups() {
        let q = parse("a OR b c");
        no_err(&q);
        assert_eq!(
            q.expr,
            Some(TermExpr::Or(vec![
                TermExpr::Word("a".into()),
                TermExpr::And(vec![TermExpr::Word("b".into()), TermExpr::Word("c".into())])
            ]))
        );
    }

    #[test]
    fn exclude() {
        let q = parse("nginx -apache -\"old config\"");
        no_err(&q);
        assert_eq!(
            q.expr,
            Some(TermExpr::And(vec![
                TermExpr::Word("nginx".into()),
                TermExpr::Not(Box::new(TermExpr::Word("apache".into()))),
                TermExpr::Not(Box::new(TermExpr::Phrase("old config".into()))),
            ]))
        );
    }

    #[test]
    fn all_fields() {
        let q = parse(
            "kind:session agent:claude-code project:anyfind ext:pdf;docx \
             size:>10mb date:today in:D:/coding role:user 关键词",
        );
        no_err(&q);
        assert!(q.filters.contains(&Filter::Kind(vec!["session".into()])));
        assert!(q.filters.contains(&Filter::Agent(vec!["claude-code".into()])));
        assert!(q.filters.contains(&Filter::Project("anyfind".into())));
        assert!(q.filters.contains(&Filter::Ext(vec!["pdf".into(), "docx".into()])));
        assert!(q.filters.contains(&Filter::Size(SizeRange { min: Some(10 << 20), max: None })));
        assert!(q.filters.iter().any(|f| matches!(f, Filter::Date(_))));
        assert!(q.filters.contains(&Filter::In("D:/coding".into())));
        assert!(q.filters.contains(&Filter::Role("user".into())));
        assert_eq!(q.expr, Some(TermExpr::Word("关键词".into())));
    }

    #[test]
    fn size_range() {
        let q = parse("size:1mb..5mb");
        no_err(&q);
        assert_eq!(
            q.filters[0],
            Filter::Size(SizeRange { min: Some(1 << 20), max: Some(5 << 20) })
        );
    }

    #[test]
    fn date_range_explicit() {
        let q = parse("date:2026-10-01..2026-10-20");
        no_err(&q);
        match &q.filters[0] {
            Filter::Date(DateRange { from, to }) => {
                assert!(from.is_some() && to.is_some());
                assert!(to.unwrap() > from.unwrap());
            }
            f => panic!("expected date, got {f:?}"),
        }
    }

    #[test]
    fn bad_field_value_reported() {
        let q = parse("kind:bogus role:root date:xyz size:big");
        assert_eq!(q.filters.len(), 0);
        assert_eq!(q.errors.len(), 4);
    }

    #[test]
    fn unknown_field_is_bare_word() {
        let q = parse("foo:bar");
        no_err(&q);
        assert_eq!(q.expr, Some(TermExpr::Word("foo:bar".into())));
    }

    #[test]
    fn unclosed_quote_reported() {
        let q = parse("\"未闭合");
        assert_eq!(q.errors.len(), 1);
        assert_eq!(q.expr, Some(TermExpr::Phrase("未闭合".into())));
    }

    #[test]
    fn fts5_build() {
        let q = parse("nginx \"代理 配置\" -apache");
        let f = fts5_query(&q).expect("fts query");
        assert_eq!(f, "\"nginx\" \"代理 配置\"");
        // 短词降级
        let q2 = parse("ab");
        assert!(fts5_query(&q2).is_none());
    }

    #[test]
    fn text_match_boolean() {
        let q = parse("nginx OR apache -deprecated");
        let e = q.expr.unwrap();
        assert!(text_matches(&e, "i run nginx here"));
        assert!(text_matches(&e, "apache server"));
        assert!(!text_matches(&e, "nginx but deprecated"));
        assert!(!text_matches(&e, "nothing"));
    }
}
