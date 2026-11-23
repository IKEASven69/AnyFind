# 统一搜索内核 HTTP API 契约 v0.2

> 2026-10-20。三路并行开发的对接契约：内核（server.py）实现方、桌面 UI 方、dsh 插件方都以此为准。
> 扩展字段允许，破坏本契约的改动必须三方同步。实现细节可协商，但端点路径、请求参数名、响应字段名固定。
> 2026-11-13 v0.2：新增 `/api/file/{list,read,stat,op}` 文件端点（见下）；旧端点文档不变。
> 2026-11-21 v0.2 补充：新增 `GET /api/file/drives` 盘符端点（见下）。

## 服务

- Rust 内核以本地 HTTP 服务运行：默认尝试 `http://127.0.0.1:8790`；桌面端检测冲突后自动改用空闲端口，并把实际地址写入 `%APPDATA%/anyfind/config.json` 的 `kernel_url` 供 dsh 插件自动发现
- 直接运行内核时可用 `--port` 指定端口；普通桌面用户无需配置地址或端口
- 无认证，仅监听 loopback
- Host 门禁（防 DNS rebinding）：所有请求的 `Host` 必须是本机回环（`127.0.0.1` / `localhost` / `[::1]`，大小写不敏感、可带端口），否则 `403 {"error"}`；正常客户端（桌面壳、dsh 插件代理、脚本）访问回环地址时天然满足
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

请求：`{"agent": "claude-code", "session_id": "...", "cwd": "C:/Users/me/projects/demo"}`

→ `{"ok": true, "command": "claude --resume xxx"}` 或 `{"ok": false, "error": "..."}`

- 语义：生成并返回该 harness 的 resume 命令；`"spawn": true` 时内核直接启动新终端执行（桌面版默认），否则仅返回命令串由调用方处理（dsh 插件用）

## GET /api/session/{session_id}

→ 会话完整元信息 + 按消息分段的原文（用于跳回原文预览页）：

```json
{"session_id": "...", "agent": "...", "project": "...", "cwd": "...",
 "messages": [{"role": "user|assistant", "ts": "...", "text": "..."}]}
```

## 文件端点（v0.2）

四个端点共享的规则（`/api/file/drives` 无 `path` 参数，不受这些约束）：

- `path`/`target` 必须是绝对路径（Windows 盘符、UNC、正/反斜杠均可），需 URL 编码；相对路径 → `400 {"error"}`
- 路径先归一再访问：`.`/`..` 词法消解、符号链接逐级跟随（深度 ≤ 32，超限 → 404 `{"error"}`）；不存在 / 不可读 → `404 {"error"}`
- `mtime` 为本地时区 ISO 字符串 `"YYYY-MM-DDTHH:MM:SS"`（与 `<Hit>.mtime` 同格式），取不到时为 `null`

### GET /api/file/list

参数：

| 参数 | 说明 |
|---|---|
| `path` | 目录绝对路径（必须） |
| `max` | 返回条目数上限，1..2000，默认 500（解析失败取默认，越界钳制） |
| `noise` | `0`（默认）隐藏噪声目录；`1` 显示 |

- 噪声目录（仅隐藏目录本身，同名文件不受影响，大小写不敏感）：`.git` `node_modules` `dist` `build` `coverage` `.idea` `target`
- 排序：目录优先，其后按名字排序（大小写不敏感，同名再按原名）

→ `200`：

```json
{"path": "D:\\coding\\xxx", "entries": [{"name": "sub", "is_dir": true, "size": 0, "mtime": "2026-11-01T10:00:00"}], "truncated": false}
```

- `path` 回显请求原值
- `size` 为字节数（目录为 0）
- `truncated`：噪声过滤并按 `max` 截断后仍有剩余条目 → `true`
- 错误：`400 {"error"}` 缺 `path` 或非绝对路径；`404 {"error"}` 不存在 / 不可读 / 不是目录

### GET /api/file/read

参数：

| 参数 | 说明 |
|---|---|
| `path` | 文件绝对路径（必须） |
| `offset` | 0 起行号，默认 0（非法/负数按 0） |
| `lines` | 读取行数，默认 30，钳制 1..500 |
| `format` | `base64` = 取字节通路（见下）；缺省或其他值 = 文本分页 |

→ `200`（文本）：

```json
{"content": "窗口内的行（\\n 拼接）", "totalLines": 120, "offset": 0, "lines": 30, "truncated": false, "binary": false}
```

- `content` 仅为该窗口；行按 `\n` 切分并剥除行尾 `\r`
- `lines` = 实际返回行数（可能小于请求值）
- `truncated` = 窗口已触及或越过文件末尾（`offset + 实际返回行数 >= totalLines`，与桌面 UI mock 语义一致）
- `totalLines` = 全文行数（空文件为 0；尾随换行不计空行）
- 二进制判定先于尺寸判定：前 8KB（或全文件）含 NUL → `200 {"binary": true}`（不带 `content` 等字段，即使文件 > 2MB 也按二进制返回）
- 超过 2MB 的非二进制文件 → `413 {"error"}`
- 错误：`400 {"error"}` 缺 `path` 或非绝对路径；`404 {"error"}` 不存在 / 是目录 / 不可读

`format=base64`（取字节通路，供 PDF/图片等二进制预览）：

```json
{"content_base64": "<标准 base64，含 = 填充>", "binary": true, "truncated": false}
```

- 整文件编码（文本与二进制通用），不与 `offset`/`lines` 组合（分页仅文本通路，该模式下忽略二者）
- `content_base64` = 整文件字节的 base64（客户端解码即原始字节，字节精确一致）
- `binary` = 前 8KB（或全文件）含 NUL 的检测结果（文本文件为 `false`，PDF/图片为 `true`）；`truncated` 恒为 `false`（整文件返回，无分页截断）
- 超过 2MB（无论是否二进制）→ `413 {"error"}`；目录 → `404 {"error"}`
- 不传 `format` 或其值非 `base64` → 走文本分页通路，行为与之前完全一致

### GET /api/file/stat

参数：`path`（绝对路径，必须）。

→ `200`：

```json
{"name": "a.txt", "is_dir": false, "size": 19, "mtime": "2026-11-01T10:00:00"}
```

- `name` = 路径最后一段（根路径如 `D:\` 回显原路径）
- 错误：`400 {"error"}` 缺 `path` 或非绝对路径；`404 {"error"}` 不存在

### GET /api/file/drives

无参数。返回本机盘符列表（文件浏览器的根导航入口）。

→ `200`：

```json
{"drives": [{"path": "C:/", "type": "fixed"}, {"path": "D:/", "type": "fixed"}, {"path": "E:/", "type": "removable"}]}
```

- Windows：`GetLogicalDrivesW` 位掩码 + `GetDriveTypeW`；`path` 形如 `"D:/"`（盘符大写 + `:/`），按盘符排序、去重
- `type` 枚举：`fixed`（本地硬盘）| `removable`（可移动盘/U 盘）| `cdrom`（光驱）| `ram`（内存盘）| `unknown`（网络盘 `DRIVE_REMOTE`、未识别 `DRIVE_UNKNOWN`、无效根 `DRIVE_NO_ROOT_DIR` 统一归入）
- 非 Windows：固定返回 `[{"path": "/", "type": "fixed"}]`
- 纯枚举端点：永远 `200`，无 4xx 分支（不访问具体路径）

### POST /api/file/op

请求体：`{"op": "...", "paths": ["..."], "target"?: "...", "newName"?: "..."}`

| op | 语义 | `paths` | `target` | `newName` |
|---|---|---|---|---|
| `rename` | 重命名（不跨目录） | 恰好 1 条旧路径 | 所在目录 | 必填 |
| `copy` | 复制到目标目录 | 1..128 条源路径 | 必填（目标目录） | — |
| `cut` | 移动到目标目录 | 1..128 条源路径 | 必填（目标目录） | — |
| `recycle` | 移入回收站（非永久删除） | 1..128 条路径 | — | — |
| `newFile` | 新建文件 | 恰好 1 条父目录 | — | 必填 |
| `newDir` | 新建目录 | 恰好 1 条父目录 | — | 必填 |

- `paths` 上限 128，超出 → `400 {"error"}`
- `newName` 校验：非空、非 `.`/`..`、无控制字符与 `<>:"/\|?*`、不以点或空格结尾、非 Windows 保留名（`CON` `PRN` `AUX` `NUL` `COM1`..`COM9` `LPT1`..`LPT9`）；违反 → `400 {"error"}`
- 目标路径做 canonicalize + 前缀 containment（`newName` 不得逃出 `target`/父目录）
- `rename` 强制「不跨目录」：`target` 词法归一后必须就是 `paths[0]` 所在目录（`.`/`..` 消解、大小写不敏感比较），跨目录或 `..` 越级 → `400 {"error"}`
- CSRF 门禁（内核无网页前端，正常客户端不带 `Origin`）：请求携带 `Origin` 头 → `403 {"error"}`（含 `file://`/沙箱 iframe 的 `null`）；`Content-Type` 非 `application/json` → `415 {"error"}`（浏览器跨站免预检简单请求只能发 `text/plain` 等，被此拦下）

→ `200`：

```json
{"ok": true, "results": [{"path": "D:\\coding\\a.txt", "ok": true}, {"path": "D:\\coding\\b.txt", "ok": false, "error": "目标已存在: ..."}]}
```

- `results` 与请求 `paths` 等长同序，`path` 回显请求原路径
- 项级失败（源不存在、目标已存在、无权限等）不改变 HTTP 状态：`200` + `ok:false` + `error`
- 请求级错误 → `4xx {"error"}`：`400`（请求体非合法 JSON / `op` 不支持 / `paths` 缺失、超限或非绝对 / 缺 `target` 或 `newName` / `newName` 非法 / 回收根路径 / `rename` 跨目录）；`403`（携带 `Origin` 的跨站请求）；`404`（`target` 或父目录不存在、不可读）；`415`（`Content-Type` 非 `application/json`）
- `501 {"error"}`：非 Windows 平台（本端点经 PowerShell 执行，仅 Windows 提供）
- `500 {"error"}`：执行器启动或输出解析失败（内核内部错误，正常环境不应出现）
- `recycle` 的对象若 canonicalize 后是根路径（`D:\`、UNC 共享根）→ `400` 拒绝

dsh 插件同源代理 `/dsh-anyfind/api/file/{list,read,stat,op,drives}` 转发本组端点，参数与响应结构同内核（代理路由由插件阶段实现，内核侧不感知）。

## 数据源归属约定

| kind | 来源 | 负责模块 |
|---|---|---|
| file | MFT 文件名索引（Windows）/ 目录扫描 | mft_index.py / scanner.py（已有） |
| content | FTS5 文档内容（已有） | scanner.py（已有） |
| session | 六家 harness 会话日志 jsonl | session_index.py（新） |
| skill | `~/.claude/skills`、`~/.config/opencode/skills` 等 + 项目级 skills 目录 | agent_files_index.py（新） |
| memory | `~/.claude/`、`~/.codex/` 记忆文件、项目 CLAUDE.md/AGENTS.md/.claude/ | agent_files_index.py（新） |
