# 统一搜索内核 HTTP API 契约 v0.1

> 2026-10-20。三路并行开发的对接契约：内核（server.py）实现方、桌面 UI 方、dsh 插件方都以此为准。
> 扩展字段允许，破坏本契约的改动必须三方同步。实现细节可协商，但端点路径、请求参数名、响应字段名固定。

## 服务

- Rust 内核以本地 HTTP 服务运行：默认尝试 `http://127.0.0.1:8790`；桌面端检测冲突后自动改用空闲端口，并把实际地址写入 `%APPDATA%/anyfind/config.json` 的 `kernel_url` 供 dsh 插件自动发现
- 直接运行内核时可用 `--port` 指定端口；普通桌面用户无需配置地址或端口
- 无认证，仅监听 loopback
- 所有响应 JSON，UTF-8

## GET /api/health

→ `{"ok": true, "version": "0.1.0", "db_path": "D:/AnyFindData/index.db", "index": {"files": N, "sessions": N, "skills": N, "memories": N}, "indexing": false}`

- `db_path` 用于确认当前实际索引位置。默认选择非系统盘的 `AnyFindData`，可用 `ANYFIND_DATA_DIR`、配置文件 `data_dir` 或 `--db` 覆盖；`names.bin` 与数据库保持同目录。
- 健康响应读取内存中的计数快照，不在请求路径重复打开数据库或执行全表统计；计数在启动和索引批次结束后刷新。

## GET /api/search

参数：

| 参数 | 说明 |
|---|---|
| `q` | 查询串，支持过滤语法：`kind:` `agent:` `date:` `ext:` `project:` 及裸关键词（可多个，AND） |
| `kinds` | 逗号分隔：`file,content,session,skill,memory`，缺省=全部 |
| `limit` | 每类返回上限，默认 20 |
| `offset` | 分页，默认 0 |

纯字段查询（无裸关键词，如 `kind:session agent:codex`）按过滤条件浏览：会话/内容/技能/记忆按 mtime 新到旧，文件按索引序；完全空查询返回空 `groups`。anyfindd 支持该语义，旧 Python 参考内核仅支持带关键词查询。

→

```json
{
  "q": "nginx",
  "took_ms": 12,
  "groups": {
    "file":    [ <Hit> ],
    "content": [ <Hit> ],
    "session": [ <Hit> ],
    "skill":   [ <Hit> ],
    "memory":  [ <Hit> ]
  },
  "truncated": {"session": true}
}
```

`<Hit>` 统一结构（各 kind 共用，不适用字段为 null）：

```json
{
  "kind": "session",
  "title": "会话标题或文件名",
  "path": "磁盘绝对路径（文件类）或 null",
  "is_dir": "file 命中时为 true/false，其他类型为 null",
  "snippet": "命中上下文片段，高亮用 <mark> 包裹",
  "mtime": "2026-10-12T08:30:00",
  "agent": "claude-code | codex | dsh | opencode | pi | cursor | null",
  "project": "项目名（从路径推断）或 null",
  "session_id": "可 resume 的会话 id 或 null",
  "cwd": "会话工作目录或 null",
  "score": 0.83
}
```

- `skill` 与 `memory` 命中：`path` 指向 SKILL.md / 记忆文件，`agent` 为该目录归属的 harness
- 高亮统一用 `<mark>`，前端不再自行计算

## POST /api/resume

带上下文开新会话（桌面版/dsh 面板"跳回会话"动作）。

请求：`{"agent": "claude-code", "session_id": "...", "cwd": "D:/coding/xxx"}`

→ `{"ok": true, "command": "claude --resume xxx"}` 或 `{"ok": false, "error": "..."}`

- 语义：生成并返回该 harness 的 resume 命令；`"spawn": true` 时内核直接启动新终端执行（桌面版默认），否则仅返回命令串由调用方处理（dsh 插件用）

## GET /api/session/{session_id}

→ 会话完整元信息 + 按消息分段的原文（用于跳回原文预览页）：

```json
{"session_id": "...", "agent": "...", "project": "...", "cwd": "...",
 "messages": [{"role": "user|assistant", "ts": "...", "text": "..."}]}
```

## 数据源归属约定

| kind | 来源 | 负责模块 |
|---|---|---|
| file | MFT 文件名索引（Windows）/ 目录扫描 | mft_index.py / scanner.py（已有） |
| content | FTS5 文档内容（已有） | scanner.py（已有） |
| session | 六家 harness 会话日志 jsonl | session_index.py（新） |
| skill | `~/.claude/skills`、`~/.config/opencode/skills` 等 + 项目级 skills 目录 | agent_files_index.py（新） |
| memory | `~/.claude/`、`~/.codex/` 记忆文件、项目 CLAUDE.md/AGENTS.md/.claude/ | agent_files_index.py（新） |
