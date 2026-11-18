//! MCP server（`anyfindd mcp` 子命令）：JSON-RPC 2.0 over stdio，换行分隔。
//! stdout 只输出协议响应（每行一个 JSON 对象）；一切日志/横幅走 stderr。
//! 手搓协议层，零新依赖（serde_json 已有）；搜索复用进程内
//! anyfind_core::search::unified_search，不起 HTTP 监听。
//!
//! 分层：`parse_request` / `dispatch` / 参数解析为纯函数（单测覆盖）；
//! 工具执行走 `ToolEnv` trait（真实实现 `ServerEnv` 持连接池与内存文件名索引，
//! 测试用 mock）。

use anyfind_core::name_index::NameIndex;
use anyfind_core::{db, file_ops, search};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// initialize 缺省协议版本（客户端带 protocolVersion 时回显其值）。
pub const PROTOCOL_VERSION_DEFAULT: &str = "2025-06-18";
/// serverInfo.name。
pub const SERVER_NAME: &str = "anyfind";

const CODE_PARSE_ERROR: i32 = -32700;
const CODE_INVALID_REQUEST: i32 = -32600;
const CODE_METHOD_NOT_FOUND: i32 = -32601;
const CODE_INVALID_PARAMS: i32 = -32602;

// ---------- JSON-RPC 请求解析（纯） ----------

/// 已解析的 JSON-RPC 请求。`id: None` 即 notification（不回响应）。
#[derive(Debug, Clone)]
pub struct RpcRequest {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

/// 解析一行请求：坏 JSON → -32700；合法 JSON 但非对象 / method 非字符串
/// → -32600（结构非法）。`id` 键缺省视为 notification。
pub fn parse_request(line: &str) -> Result<RpcRequest, (i32, String)> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| (CODE_PARSE_ERROR, format!("Parse error: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| (CODE_INVALID_REQUEST, "Invalid Request: 请求必须是 JSON 对象".into()))?;
    let method = obj
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or_else(|| (CODE_INVALID_REQUEST, "Invalid Request: method 必须是字符串".into()))?
        .to_string();
    Ok(RpcRequest {
        id: obj.get("id").cloned(),
        method,
        params: obj.get("params").cloned().unwrap_or(Value::Null),
    })
}

// ---------- 响应构造 ----------

fn ok_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// 工具结果统一形态：{content:[{type:"text", text}], isError}。
/// isError 仅在工具级失败（执行期错误）时 true。
fn tool_result(payload: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

// ---------- 工具参数解析（纯） ----------

/// anyfind_search 参数。kinds 为空表示全部类别（与 HTTP /api/search 同语义）。
#[derive(Debug, Clone, PartialEq)]
pub struct SearchParams {
    pub q: String,
    pub kinds: Vec<String>,
    pub limit: usize,
}

pub const SEARCH_LIMIT_DEFAULT: usize = 20;
pub const SEARCH_LIMIT_MAX: usize = 50;

/// 解析 anyfind_search 的 arguments：q 必填非空字符串；kinds 可选字符串数组
/// （非字符串元素报错，未知类别过滤掉，全过滤后为空 → 全部类别）；limit 可选
/// 整数，钳制 1..=50，缺省 20。
pub fn parse_search_params(args: &Value) -> Result<SearchParams, String> {
    let q = args
        .get("q")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "参数错误：q 必须是非空字符串".to_string())?
        .trim()
        .to_string();
    if q.is_empty() {
        return Err("参数错误：q 不能为空".into());
    }
    let kinds: Vec<String> = match args.get("kinds") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            let mut ks = Vec::new();
            for k in a {
                let s = k
                    .as_str()
                    .ok_or_else(|| "参数错误：kinds 必须是字符串数组".to_string())?;
                if search::ALL_KINDS.contains(&s) {
                    ks.push(s.to_string());
                }
            }
            ks
        }
        _ => return Err("参数错误：kinds 必须是字符串数组".into()),
    };
    let limit = match args.get("limit") {
        None | Some(Value::Null) => SEARCH_LIMIT_DEFAULT,
        Some(v) => {
            let n = v
                .as_i64()
                .ok_or_else(|| "参数错误：limit 必须是整数".to_string())?;
            (n.max(1) as usize).min(SEARCH_LIMIT_MAX)
        }
    };
    Ok(SearchParams { q, kinds, limit })
}

/// anyfind_file_read 参数。
#[derive(Debug, Clone, PartialEq)]
pub struct FileReadParams {
    pub path: String,
    pub lines: usize,
}

/// 解析 anyfind_file_read 的 arguments：path 必填非空字符串；lines 可选整数，
/// 钳制 1..=500，缺省 30（与内核 /api/file/read 同窗口）。
pub fn parse_file_read_params(args: &Value) -> Result<FileReadParams, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "参数错误：path 必须是非空字符串".to_string())?
        .to_string();
    let lines = match args.get("lines") {
        None | Some(Value::Null) => file_ops::READ_LINES_DEFAULT,
        Some(v) => {
            let n = v
                .as_i64()
                .ok_or_else(|| "参数错误：lines 必须是整数".to_string())?;
            (n.max(1) as usize).min(file_ops::READ_LINES_MAX)
        }
    };
    Ok(FileReadParams { path, lines })
}

/// 解析 anyfind_file_stat 的 arguments：path 必填非空字符串。
pub fn parse_file_stat_params(args: &Value) -> Result<String, String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| "参数错误：path 必须是非空字符串".to_string())
}

// ---------- 工具执行环境 ----------

/// 工具执行抽象：dispatch 依赖它，测试注入 mock。
pub trait ToolEnv {
    fn search(&self, p: &SearchParams) -> Result<Value, String>;
    fn file_read(&self, p: &FileReadParams) -> Result<Value, String>;
    fn file_stat(&self, path: &str) -> Result<Value, String>;
}

// ---------- 分派（纯） ----------

/// 分派单个请求。返回完整响应对象；notification（无 id）与
/// `notifications/*` 一律返回 None（不回响应）。
pub fn dispatch(env: &dyn ToolEnv, req: &RpcRequest) -> Option<Value> {
    // MCP 通知（notifications/initialized、notifications/cancelled 等）无响应；
    // 客户端不守约定带上 id 也不回，与规范语义一致。
    if req.method.starts_with("notifications/") {
        return None;
    }
    let Some(id) = req.id.clone() else {
        return None; // notification：未知方法同样不回
    };
    let resp = match req.method.as_str() {
        "initialize" => ok_response(id, initialize_result(&req.params)),
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, tools_list()),
        "tools/call" => match call_tool(env, &req.params) {
            Ok(result) => ok_response(id, result),
            Err(msg) => error_response(id, CODE_INVALID_PARAMS, &msg),
        },
        _ => error_response(id, CODE_METHOD_NOT_FOUND, &format!("未知方法: {}", req.method)),
    };
    Some(resp)
}

/// initialize 结果：protocolVersion 回显客户端请求值（缺省 2025-06-18），
/// capabilities 仅声明 tools，serverInfo 带 内核版本。
fn initialize_result(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION_DEFAULT);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
    })
}

/// tools/list 结果：3 个工具，inputSchema 用 JSON Schema 子集。
fn tools_list() -> Value {
    json!({"tools": [
        {
            "name": "anyfind_search",
            "description": "AnyFind 统一搜索：文件名 / 文档内容 / 六家 harness 会话原文 / skills / agent 记忆。\
支持 DSL：裸词 AND、\"短语\"、OR、-排除，kind:/agent:/project:/ext:/size:/date:/in:/role: 字段过滤",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q": {"type": "string", "description": "查询词（支持 AnyFind DSL）"},
                    "kinds": {
                        "type": "array",
                        "items": {"type": "string", "enum": search::ALL_KINDS.to_vec()},
                        "description": "限定类别（file/content/session/skill/memory），缺省全部"
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": SEARCH_LIMIT_MAX,
                        "description": "每组命中上限（1..50，缺省 20）"
                    }
                },
                "required": ["q"]
            }
        },
        {
            "name": "anyfind_file_read",
            "description": "按行窗口读取文本文件（绝对路径；前 8KB 含 NUL 判二进制；>2MB 只取前 2MB 并置 truncated）",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "绝对路径"},
                    "lines": {
                        "type": "integer", "minimum": 1, "maximum": file_ops::READ_LINES_MAX,
                        "description": "读取行数（1..500，缺省 30）"
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "anyfind_file_stat",
            "description": "文件 / 目录元信息：名称、类型、大小、修改时间",
            "inputSchema": {
                "type": "object",
                "properties": {"path": {"type": "string", "description": "绝对路径"}},
                "required": ["path"]
            }
        }
    ]})
}

/// tools/call 分派：未知工具 / 坏参数 → Err（→ -32602）；工具级失败
/// （路径不存在等执行期错误）以 isError:true 的正常结果返回，错误文本
/// 在 content 里——与 HTTP 4xx 契约错误对应，但不是协议层错误。
fn call_tool(env: &dyn ToolEnv, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "参数错误：name 必须是字符串".to_string())?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let args = args.as_object().cloned().unwrap_or_default();
    let args = Value::Object(args);
    // 执行结果 → 统一工具结果形态；Err 是工具级失败（isError:true）。
    let finish = |r: Result<Value, String>| match r {
        Ok(payload) => tool_result(&payload, false),
        Err(msg) => tool_result(&json!({"error": msg}), true),
    };
    match name {
        "anyfind_search" => {
            let p = parse_search_params(&args)?;
            Ok(finish(env.search(&p)))
        }
        "anyfind_file_read" => {
            let p = parse_file_read_params(&args)?;
            Ok(finish(env.file_read(&p)))
        }
        "anyfind_file_stat" => {
            let path = parse_file_stat_params(&args)?;
            Ok(finish(env.file_stat(&path)))
        }
        other => Err(format!("未知工具: {other}")),
    }
}

/// 单条消息处理（行级）：空行跳过；解析失败回错误响应（id:null）；
/// notification 无响应。返回待写入 stdout 的一行 JSON。
pub fn handle_message(env: &dyn ToolEnv, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let resp = match parse_request(line) {
        Err((code, msg)) => error_response(Value::Null, code, &msg),
        Ok(req) => dispatch(env, &req)?,
    };
    serde_json::to_string(&resp).ok()
}

// ---------- 真实工具实现（复用内核 unified_search / file_ops） ----------

type SharedNames = Arc<RwLock<NameIndex>>;

/// 进程内工具环境：与 HTTP daemon 同一套 unified_search 与文件读取语义。
pub struct ServerEnv {
    pool: Arc<db::Pool>,
    names: SharedNames,
}

impl ServerEnv {
    pub fn new(pool: Arc<db::Pool>, names: SharedNames) -> Self {
        ServerEnv { pool, names }
    }

    /// anyfind_search：五组命中按固定顺序摊平成单一数组（每组 ≤ limit）。
    fn search_impl(&self, p: &SearchParams) -> Value {
        let kinds: Vec<String> = if p.kinds.is_empty() {
            search::ALL_KINDS.iter().map(|s| s.to_string()).collect()
        } else {
            p.kinds.clone()
        };
        let res = {
            let guard = self.names.read().ok();
            search::unified_search(&self.pool, guard.as_deref(), &p.q, &kinds, p.limit, 0)
        };
        let mut hits: Vec<Value> = Vec::new();
        for kind in search::ALL_KINDS {
            if let Some(arr) = res.groups.get(kind).and_then(|v| v.as_array()) {
                hits.extend(arr.iter().cloned());
            }
        }
        Value::Array(hits)
    }

    /// anyfind_file_read：require_absolute → canonicalize → 元数据 →
    /// ≤2MB 读取 → 二进制嗅探 → 行窗口（offset 固定 0）。
    fn file_read_impl(&self, p: &FileReadParams) -> Result<Value, String> {
        let path = file_ops::require_absolute(&p.path)?;
        let resolved = file_ops::canonicalize_limited(&path)?;
        let md = std::fs::metadata(&resolved)
            .map_err(|e| format!("无法读取 {}: {e}", p.path))?;
        if md.is_dir() {
            return Err(format!("无法按文本读取目录: {}", p.path));
        }
        let overflow = md.len() > file_ops::READ_SIZE_MAX;
        let mut bytes = Vec::new();
        {
            use std::io::Read;
            let f = std::fs::File::open(&resolved)
                .map_err(|e| format!("无法读取 {}: {e}", p.path))?;
            f.take(file_ops::READ_SIZE_MAX)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("无法读取 {}: {e}", p.path))?;
        }
        if file_ops::is_binary_prefix(&bytes) {
            return Ok(json!({
                "content": "", "totalLines": 0, "truncated": false, "binary": true,
            }));
        }
        let text = String::from_utf8_lossy(&bytes);
        let page = file_ops::paginate(&text, 0, p.lines);
        Ok(json!({
            "content": page.lines.join("\n"),
            "totalLines": page.total,
            "truncated": page.truncated || overflow,
        }))
    }

    /// anyfind_file_stat：与 /api/file/stat 同字段。
    fn file_stat_impl(&self, raw: &str) -> Result<Value, String> {
        let path = file_ops::require_absolute(raw)?;
        let resolved = file_ops::canonicalize_limited(&path)?;
        let md = std::fs::metadata(&resolved)
            .map_err(|e| format!("无法读取 {raw}: {e}"))?;
        let name = resolved
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_string());
        Ok(json!({
            "path": raw,
            "name": name,
            "is_dir": md.is_dir(),
            "size": md.len(),
            "mtime": crate::entry_mtime(&md),
        }))
    }
}

impl ToolEnv for ServerEnv {
    fn search(&self, p: &SearchParams) -> Result<Value, String> {
        Ok(self.search_impl(p))
    }
    fn file_read(&self, p: &FileReadParams) -> Result<Value, String> {
        self.file_read_impl(p)
    }
    fn file_stat(&self, path: &str) -> Result<Value, String> {
        self.file_stat_impl(path)
    }
}

// ---------- stdio 服务循环 ----------

/// 主循环：stdin 逐行读，stdout 只写协议响应（每条后 flush）。
/// stdout 写失败（客户端断开）即退出。
pub fn serve(env: &dyn ToolEnv) -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(resp) = handle_message(env, &line) {
            if writeln!(out, "{resp}").is_err() || out.flush().is_err() {
                break;
            }
        }
    }
    0
}

// ---------- 子命令入口 ----------

/// `anyfindd mcp [--db <path>] [--no-index]`。
#[derive(Debug, Clone)]
pub struct McpArgs {
    pub db: PathBuf,
    pub no_index: bool,
}

/// 解析 mcp 子命令参数（未知参数告警到 stderr 后忽略，不影响协议通道）。
pub fn parse_mcp_args(args: &[String]) -> McpArgs {
    let mut out = McpArgs {
        db: anyfind_core::paths::db_path(),
        no_index: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => {
                if let Some(v) = it.next() {
                    out.db = PathBuf::from(v);
                }
            }
            "--no-index" => out.no_index = true,
            other => eprintln!("[anyfindd/mcp] 忽略未知参数: {other}"),
        }
    }
    out
}

/// MCP 模式入口：复用 HTTP 主路径的初始化（main.rs:1363 起）——
/// ANYFIND_DATA_DIR 跟随 --db、DbInstanceGuard 单写者锁、names.bin 冷启动
/// 装载、db 打开与连接池；之后以 stdio 服务循环替代 HTTP 监听。
///
/// 索引（写）模式持实例锁；锁被占用（如桌面 anyfindd 在跑）或 --no-index
/// 时降级为只读——不开索引线程、不写库，可与桌面内核共存（WAL 多进程读安全）。
/// 返回进程退出码。
pub fn run_mcp(args: McpArgs) -> i32 {
    // names.bin 必须跟随 --db，而不是继续写入默认配置盘（同 main）。
    if let Some(dir) = args.db.parent() {
        let _ = std::fs::create_dir_all(dir);
        std::env::set_var("ANYFIND_DATA_DIR", dir);
    }
    let mut indexing = !args.no_index;
    let _guard = if indexing {
        match crate::DbInstanceGuard::acquire(&args.db) {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("[anyfindd/mcp] {e}；降级只读模式（不启动索引，搜索继续）");
                indexing = false;
                None
            }
        }
    } else {
        None
    };

    // 冷启动：先装 names.bin（内存文件名索引），后台索引完成后再重建刷新。
    let names: SharedNames = Arc::new(RwLock::new(NameIndex::default()));
    let blob = crate::names_blob_path();
    if let Some(idx) = NameIndex::load_blob(&blob) {
        eprintln!(
            "[anyfindd/mcp] names.bin 装载 {} 条: {}",
            idx.entries.len(),
            blob.display()
        );
        if let Ok(mut g) = names.write() {
            *g = idx;
        }
    }
    if let Some(conn) = crate::open_db(&args.db) {
        drop(conn);
        eprintln!("[anyfindd/mcp] 索引库: {}", args.db.display());
    } else {
        eprintln!("[anyfindd/mcp] 无法打开索引库: {}", args.db.display());
        return 1;
    }
    let pool = Arc::new(db::Pool::new(&args.db, 6));

    if indexing {
        let stats = Arc::new(crate::IndexStats::default());
        let unified_indexing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let file_indexing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::start_unified_index_loop(args.db.clone(), unified_indexing, stats.clone());
        crate::start_file_index(
            &crate::Args {
                port: 8790,
                root: None,
                drives: None,
                no_index: false,
                no_watch: false,
                content_roots: Vec::new(),
                db: args.db.clone(),
            },
            file_indexing,
            names.clone(),
            stats,
        );
        // 周期性落盘 names.bin（USN 增量只改内存；同 HTTP 主路径）
        let names_bg = names.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if let Ok(g) = names_bg.read() {
                if !g.entries.is_empty() {
                    let _ = g.save_blob(&crate::names_blob_path());
                }
            }
        });
    }
    eprintln!(
        "[anyfindd/mcp] MCP server 就绪（stdio, protocolVersion 缺省 {PROTOCOL_VERSION_DEFAULT}）"
    );

    let env = ServerEnv::new(pool, names);
    serve(&env)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录调用的 mock 工具环境。
    struct MockEnv {
        fail_read: bool,
    }
    impl ToolEnv for MockEnv {
        fn search(&self, p: &SearchParams) -> Result<Value, String> {
            Ok(json!([{"kind": "file", "title": p.q, "path": "D:/x"}]))
        }
        fn file_read(&self, _p: &FileReadParams) -> Result<Value, String> {
            if self.fail_read {
                Err("文件不存在".into())
            } else {
                Ok(json!({"content": "hi", "totalLines": 1, "truncated": false}))
            }
        }
        fn file_stat(&self, _path: &str) -> Result<Value, String> {
            Ok(json!({"name": "x.txt", "is_dir": false, "size": 2}))
        }
    }

    fn req(method: &str, params: Value, id: Option<Value>) -> RpcRequest {
        RpcRequest {
            id,
            method: method.into(),
            params,
        }
    }

    // ---- 解析 ----

    #[test]
    fn parse_request_extracts_method_params_and_optional_id() {
        let r = parse_request(r#"{"jsonrpc":"2.0","id":7,"method":"ping","params":{"a":1}}"#)
            .unwrap();
        assert_eq!(r.method, "ping");
        assert_eq!(r.id, Some(json!(7)));
        assert_eq!(r.params["a"], 1);
        // 无 id → notification
        let r = parse_request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert_eq!(r.id, None);
        // 缺 params → Null
        let r = parse_request(r#"{"jsonrpc":"2.0","id":"s","method":"ping"}"#).unwrap();
        assert_eq!(r.params, Value::Null);
    }

    #[test]
    fn parse_request_bad_json_is_32700() {
        let (code, msg) = parse_request("{not json").unwrap_err();
        assert_eq!(code, -32700);
        assert!(msg.contains("Parse error"));
        // 空字符串同样 -32700
        assert_eq!(parse_request("   ").unwrap_err().0, -32700);
    }

    #[test]
    fn parse_request_non_object_or_bad_method_is_32600() {
        // JSON 合法但不是对象
        assert_eq!(parse_request("[1,2]").unwrap_err().0, -32600);
        // method 缺失 / 非字符串
        assert_eq!(parse_request(r#"{"id":1}"#).unwrap_err().0, -32600);
        assert_eq!(parse_request(r#"{"id":1,"method":5}"#).unwrap_err().0, -32600);
    }

    // ---- initialize / ping / tools/list ----

    #[test]
    fn initialize_echoes_client_protocol_version_with_default() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(
            &env,
            &req("initialize", json!({"protocolVersion": "2025-11-25"}), Some(json!(1))),
        )
        .unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(r["result"]["capabilities"]["tools"], json!({}));
        assert_eq!(r["result"]["serverInfo"]["name"], "anyfind");
        assert_eq!(
            r["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        // 缺省版本
        let r = dispatch(&env, &req("initialize", json!({}), Some(json!(1)))).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn ping_returns_empty_object_result() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(&env, &req("ping", Value::Null, Some(json!("x")))).unwrap();
        assert_eq!(r["id"], "x");
        assert_eq!(r["result"], json!({}));
        assert!(r.get("error").is_none());
    }

    #[test]
    fn tools_list_exposes_three_tools_with_schemas() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(&env, &req("tools/list", Value::Null, Some(json!(1)))).unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["anyfind_search", "anyfind_file_read", "anyfind_file_stat"]
        );
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].as_str().unwrap().len() > 10);
        }
        let search = &tools[0];
        assert_eq!(search["inputSchema"]["required"], json!(["q"]));
        assert_eq!(
            search["inputSchema"]["properties"]["kinds"]["items"]["enum"],
            json!(["file", "content", "session", "skill", "memory"])
        );
        assert_eq!(search["inputSchema"]["properties"]["limit"]["maximum"], 50);
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["path"]));
        assert_eq!(
            tools[1]["inputSchema"]["properties"]["lines"]["maximum"],
            500
        );
    }

    // ---- notification / 未知方法 ----

    #[test]
    fn notifications_never_get_responses() {
        let env = MockEnv { fail_read: false };
        assert_eq!(
            dispatch(&env, &req("notifications/initialized", Value::Null, None)),
            None
        );
        // 无 id 的未知方法同样是 notification → 不回
        assert_eq!(dispatch(&env, &req("no/such/method", Value::Null, None)), None);
    }

    #[test]
    fn unknown_method_with_id_is_32601() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(&env, &req("resources/list", Value::Null, Some(json!(3)))).unwrap();
        assert_eq!(r["id"], 3);
        assert_eq!(r["error"]["code"], -32601);
        assert!(r["error"]["message"].as_str().unwrap().contains("resources/list"));
        assert!(r.get("result").is_none());
    }

    // ---- tools/call 错误码 ----

    #[test]
    fn unknown_tool_is_32602() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(
            &env,
            &req("tools/call", json!({"name": "nope", "arguments": {}}), Some(json!(1))),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32602);
        assert!(r["error"]["message"].as_str().unwrap().contains("未知工具"));
        // name 缺失同样 -32602
        let r = dispatch(&env, &req("tools/call", json!({"arguments": {}}), Some(json!(1))))
            .unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn bad_tool_params_are_32602() {
        let env = MockEnv { fail_read: false };
        let cases = [
            json!({"name": "anyfind_search", "arguments": {}}),               // 缺 q
            json!({"name": "anyfind_search", "arguments": {"q": 5}}),          // q 非字符串
            json!({"name": "anyfind_search", "arguments": {"q": "  "}}),       // q 空白
            json!({"name": "anyfind_search", "arguments": {"q": "a", "limit": "20"}}), // limit 字符串
            json!({"name": "anyfind_search", "arguments": {"q": "a", "kinds": "file"}}), // kinds 非数组
            json!({"name": "anyfind_search", "arguments": {"q": "a", "kinds": ["file", 3]}}), // 元素非字符串
            json!({"name": "anyfind_file_read", "arguments": {}}),             // 缺 path
            json!({"name": "anyfind_file_read", "arguments": {"path": "D:/x", "lines": 1.5}}), // 非整数
            json!({"name": "anyfind_file_stat", "arguments": {"path": ""}}),   // 空 path
        ];
        for c in cases {
            let r = dispatch(&env, &req("tools/call", c.clone(), Some(json!(1)))).unwrap();
            assert_eq!(r["error"]["code"], -32602, "case: {c}");
        }
    }

    // ---- 参数钳制 ----

    #[test]
    fn search_limit_clamps_to_1_50_default_20() {
        let p = parse_search_params(&json!({"q": "a"})).unwrap();
        assert_eq!(p.limit, 20);
        assert!(p.kinds.is_empty()); // 空 = 全部
        let p = parse_search_params(&json!({"q": "a", "limit": 0})).unwrap();
        assert_eq!(p.limit, 1);
        let p = parse_search_params(&json!({"q": "a", "limit": 9999})).unwrap();
        assert_eq!(p.limit, 50);
        let p = parse_search_params(&json!({"q": "a", "limit": -5})).unwrap();
        assert_eq!(p.limit, 1);
        let p = parse_search_params(&json!({"q": "a", "limit": 7})).unwrap();
        assert_eq!(p.limit, 7);
    }

    #[test]
    fn search_kinds_filters_unknown_and_keeps_valid() {
        let p = parse_search_params(&json!({"q": "a", "kinds": ["file", "bogus", "session"]}))
            .unwrap();
        assert_eq!(p.kinds, ["file", "session"]);
        // 全未知 → 空数组（= 全部类别，与 HTTP /api/search 同语义）
        let p = parse_search_params(&json!({"q": "a", "kinds": ["bogus"]})).unwrap();
        assert!(p.kinds.is_empty());
    }

    #[test]
    fn file_read_lines_clamps_to_1_500_default_30() {
        let p = parse_file_read_params(&json!({"path": "D:/x"})).unwrap();
        assert_eq!(p.lines, 30);
        assert_eq!(p.path, "D:/x");
        let p = parse_file_read_params(&json!({"path": "D:/x", "lines": 0})).unwrap();
        assert_eq!(p.lines, 1);
        let p = parse_file_read_params(&json!({"path": "D:/x", "lines": 100000})).unwrap();
        assert_eq!(p.lines, 500);
    }

    // ---- 结果形态 ----

    #[test]
    fn tool_success_envelope_is_text_content_with_is_error_false() {
        let env = MockEnv { fail_read: false };
        let r = dispatch(
            &env,
            &req(
                "tools/call",
                json!({"name": "anyfind_search", "arguments": {"q": "hello"}}),
                Some(json!(9)),
            ),
        )
        .unwrap();
        assert_eq!(r["id"], 9);
        let content = r["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(r["result"]["isError"], false);
        // text 本身是合法 JSON（命中数组）
        let payload: Value =
            serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload[0]["title"], "hello");
    }

    #[test]
    fn tool_level_failure_returns_is_error_true_not_protocol_error() {
        let env = MockEnv { fail_read: true };
        let r = dispatch(
            &env,
            &req(
                "tools/call",
                json!({"name": "anyfind_file_read", "arguments": {"path": "D:/gone"}}),
                Some(json!(2)),
            ),
        )
        .unwrap();
        // 工具级失败：HTTP 200 语义，错误在 content + isError:true，不是 -32602
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["error"], "文件不存在");
    }

    #[test]
    fn response_id_echoes_string_and_number_ids() {
        let env = MockEnv { fail_read: false };
        for id in [json!(42), json!("abc-1"), json!(0)] {
            let r = dispatch(&env, &req("ping", Value::Null, Some(id.clone()))).unwrap();
            assert_eq!(r["id"], id);
            assert_eq!(r["jsonrpc"], "2.0");
        }
    }

    // ---- 行级处理 ----

    #[test]
    fn handle_message_round_trips_line_protocol() {
        let env = MockEnv { fail_read: false };
        // 正常请求 → 单行 JSON 响应
        let out = handle_message(
            &env,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\r\n",
        )
        .unwrap();
        assert!(!out.contains('\n'));
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["tools"].as_array().unwrap().len(), 3);
        // notification → 无输出
        assert_eq!(
            handle_message(
                &env,
                "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}"
            ),
            None
        );
        // 空行 → 无输出
        assert_eq!(handle_message(&env, "   "), None);
        // 坏 JSON → id:null + -32700
        let out = handle_message(&env, "{oops").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], Value::Null);
        assert_eq!(v["error"]["code"], -32700);
    }

    // ---- ServerEnv 真实执行（临时目录 + 真实文件） ----

    fn temp_env() -> (ServerEnv, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "anyfind-mcp-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = Arc::new(db::Pool::new(&dir.join("index.db"), 1));
        let names = Arc::new(RwLock::new(NameIndex::default()));
        (ServerEnv::new(pool, names), dir)
    }

    #[test]
    fn server_env_search_on_empty_db_returns_empty_array() {
        let (env, dir) = temp_env();
        let p = SearchParams { q: "anything".into(), kinds: vec![], limit: 20 };
        let hits = env.search(&p).unwrap();
        assert_eq!(hits, json!([]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn server_env_file_read_and_stat_on_real_file() {
        let (env, dir) = temp_env();
        let f = dir.join("sample.txt");
        std::fs::write(&f, "line1\nline2\nline3\n").unwrap();
        let raw = f.to_string_lossy().replace('/', "\\");
        // 默认 30 行 → 全量读取，totalLines=3（尾随换行不产生空行）；
        // truncated 与 /api/file/read 同语义：窗口触及文件末尾即 true
        let r = env
            .file_read(&FileReadParams { path: raw.clone(), lines: 30 })
            .unwrap();
        assert_eq!(r["content"], "line1\nline2\nline3");
        assert_eq!(r["totalLines"], 3);
        assert_eq!(r["truncated"], true);
        // 窗口 2 行（3 行文件）→ 未触及末尾，truncated=false
        let r = env
            .file_read(&FileReadParams { path: raw.clone(), lines: 2 })
            .unwrap();
        assert_eq!(r["content"], "line1\nline2");
        assert_eq!(r["truncated"], false);
        // stat
        let s = env.file_stat(&raw).unwrap();
        assert_eq!(s["name"], "sample.txt");
        assert_eq!(s["is_dir"], false);
        assert_eq!(s["size"], 18);
        assert_eq!(s["path"], raw);
        assert!(s["mtime"].as_str().is_some());
        // 相对路径 → 工具级失败（isError 语义，错误文本可读）
        let err = env.file_read(&FileReadParams { path: "relative/x".into(), lines: 5 });
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
