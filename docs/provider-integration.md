# Provider 接入指南：把 AnyFind 当检索后端

给记忆 / 知识库 / 工作区类插件的作者：你的插件需要一个「搜本机全部历史」的检索后端时，
AnyFind 内核（anyfindd）就是那个后端——一个索引统一文件名 / 文档内容 / 六家 AI harness
会话原文 / skills / agent 记忆，常用词毫秒级返回。本文覆盖三条接入通道、HTTP 契约速查、
Provider 形态伪代码与能力边界。

- HTTP 契约全文：[api-contract.md](api-contract.md)
- stdio MCP server 实现：`core/anyfindd/src/mcp.rs`（工具定义见 mcp.rs:218-264 `tools_list`）
- dsh 宿主插件参考实现：`apps/dsh-plugin/src/index.ts:138-203`（宿主内 MCP 注册段）

## 0. 通道总览

| 通道 | 形态 | 适合谁 | 进程拓扑 |
|---|---|---|---|
| A. 宿主内 MCP | `ctx.mcpManager.register('anyfind', entry)`（dsh 插件侧） | dsh 宿主里的插件 | 宿主拉起 `anyfindd mcp` stdio 子进程，工具挂进宿主 MCP 面 |
| B. 独立 stdio MCP | 客户端配置 `anyfindd mcp` 为 MCP server | 任意 MCP 客户端（Claude Code / Codex / Cursor…） | 客户端自己拉起并持有子进程 |
| C. HTTP 直连 | `GET http://127.0.0.1:8790/api/search` 等 | 任何语言的服务 / 脚本 / 面板 | 桌面端 anyfindd 常驻（loopback，无认证） |

三通道同一内核、同一索引库：A/B 走 `anyfindd mcp`（搜索在子进程内直读库，不起 HTTP
监听，见 mcp.rs:1-8）；C 走桌面端 HTTP daemon。**A 与 B 可同时存在**（多进程 WAL 读安全），
但 A 的注册名（`anyfind`）在单个 dsh 宿主内是排他的——与 configured 条目或其他注册重名
会在注册时抛错（dsh-mcp-manager src/index.ts:288 起 `register` 的抛错契约）。

## 1. 通道 A：宿主内 MCP（dsh 插件作者）

dsh 宿主装有 `@deepseek-ai/dsh-mcp-manager`（npm latest `0.2.2-rc.2`）时，`Context` 上
有 `mcpManager` 服务（模块增强，dsh-mcp-manager src/index.ts:181-185）。插件不需要写
任何 MCP 配置文件，调一次 `register` 即可：

```ts
import type { Context } from '@deepseek-ai/cordis'
import type {} from '@deepseek-ai/dsh-mcp-manager'   // 拉起 Context.mcpManager 类型 merge

export function apply(ctx: Context): void {
  // 服务缺席（宿主未装 mcp-manager）时回调不触发，插件其余功能不受影响
  ctx.inject(['mcpManager'], (mc) => {
    mc.mcpManager.register('anyfind', {
      transport: 'stdio',
      command: 'C:/…/anyfindd.exe',        // 绝对路径稳妥；裸名走 PATH 解析
      args: ['mcp', '--no-index'],          // 只读共享索引，写者留给桌面端
    })
    // 返回幂等 disposer；registration 随本插件生命周期存续——插件卸载时
    // manager 自动 unmount 并撤回该 server 的全部工具（README「Registering
    // a server from a plugin」节），无需手动 dispose。
  })
}
```

要点（对齐 dsh-mcp-manager src/index.ts:279-297 的 `register` 实现语义）：

- `name`（此处 `anyfind`）同时是宿主工具 namespace 与去重键：与 configured 条目或其他
  注册重名、entry 非法（stdio 缺 `command`、stdio 带了 `url`/`headers` 等，校验规则见
  src/index.ts:127-149 `validateEntries`）、manager 已 disposed 时**同步抛错**（`register`
  签名与契约见 src/index.ts:288）；
- entry 类型 `McpServerRegistration = Omit<EntryInput, 'enabled'>`（src/index.ts:83）——
  注册即 enabled，没有「禁用的注册」；
- server 名合法性 `/^[A-Za-z0-9_-]{1,32}$/`（dsh-mcp-client src/index.ts:52
  `SERVER_NAME_PATTERN`）。

dsh-anyfind 插件已内置此通道（`apps/dsh-plugin/src/index.ts:575-579` 注入，`:186-203`
注册），可执行路径自动发现（`:157-166` 候选：桌面端 sidecar → 开发仓构建产物；
`:171-178` `discoverAnyfindd`），config（宿主 settings 写入的插件配置）可覆盖：

```yaml
# 插件 config 字段（Config interface，apps/dsh-plugin/src/index.ts:32-41）
endpoint: http://127.0.0.1:8790   # HTTP 通道内核地址（缺省自动发现）
mcp: true                          # 宿主内 MCP 注册总开关（缺省开）
mcpCommand: C:/apps/anyfindd.exe   # 缺省自动发现；显式指定则不探测存在
mcpArgs: [mcp, --no-index]         # 缺省只读；需 dsh 独立建库时去掉 --no-index
```

## 2. 通道 B：独立 stdio MCP（任意 MCP 客户端）

`anyfindd mcp` 是一个标准 JSON-RPC 2.0 over stdio 的 MCP server（每行一个 JSON 对象；
stdout 只有协议响应，日志走 stderr，mcp.rs:1-8）。客户端配置示例：

```json
{
  "mcpServers": {
    "anyfind": {
      "command": "C:/…/anyfindd.exe",
      "args": ["mcp", "--no-index"]
    }
  }
}
```

- `--no-index`：只读模式——不拿写者锁、不起索引线程，可与桌面内核共存（WAL 多进程读
  安全，mcp.rs:463-487）。**推荐默认加**；
- 不加 `--no-index`：该子进程尝试成为写者（持实例锁 + 后台增量索引 + 周期落盘
  names.bin，mcp.rs:476-541）；桌面端在跑时锁被占，自动降级只读并打 stderr 日志；
- `--db <path>` 指定独立库（缺省跟随 `ANYFIND_DATA_DIR` / 默认数据盘）。

暴露的三个工具（`initialize` → `tools/list` 实测返回，与 mcp.rs:218-264 一致）：

| 工具 | 参数 | 语义 |
|---|---|---|
| `anyfind_search` | `q`（必填，DSL）；`kinds`（可选数组：file/content/session/skill/memory，未知类别过滤，全空=全部）；`limit`（1..50，缺省 20） | 五组命中按固定顺序摊平成单一数组（mcp.rs:329-347） |
| `anyfind_file_read` | `path`（必填绝对路径）；`lines`（1..500，缺省 30） | 行窗口读取；前 8KB 含 NUL 判二进制；>2MB 只取前 2MB 并置 truncated（mcp.rs:349-381） |
| `anyfind_file_stat` | `path`（必填） | `{path, name, is_dir, size, mtime}`（mcp.rs:383-400） |

工具级失败（路径不存在等）以 `isError:true` 的正常结果返回、错误文本在 content——不是
协议层错误（mcp.rs:267-298）；坏参数才是 `-32602`。

## 3. 通道 C：HTTP 端点速查

内核默认 `http://127.0.0.1:8790`（loopback、无认证；实际端口看
`%APPDATA%/anyfind/config.json` 的 `kernel_url`）。

### GET /api/search（主检索端点）

| 参数 | 说明 |
|---|---|
| `q` | 查询串，支持 DSL（见下）；纯字段查询=按条件浏览 |
| `kinds` | 逗号分隔：`file,content,session,skill,memory`，缺省全部 |
| `limit` | 每类返回上限，默认 20 |
| `offset` | 分页，默认 0 |

响应：`{ q, took_ms, groups: { file, content, session, skill, memory }, truncated }`，
每组是 `<Hit>` 数组，统一结构 `{kind, title, path, is_dir, snippet, mtime, agent,
project, session_id, cwd, score}`（snippet 内高亮用 `<mark>` 包裹；契约细节见
api-contract.md「GET /api/search」节）。

**DSL 要点（v1）**：裸词 AND；`"短语"`；`OR`；`-排除`；字段过滤 `kind:` `agent:`
`project:` `ext:` `size:` `date:` `in:` `role:`。例：`kind:session agent:codex 构建踩坑`。

### 其余端点一览

| 端点 | 方法 | 用途 |
|---|---|---|
| `/api/health` | GET | 存活与索引状态（Provider 探活用它） |
| `/api/session/{id}` | GET | 单会话详情 |
| `/api/resume` | POST | 取回可 resume 的会话负载（agent + session_id） |
| `/api/file/list` | GET | 目录列举（噪声过滤 + 目录优先 + 截断） |
| `/api/file/read` | GET | 行分页读文本（同通道 B 的 read 语义） |
| `/api/file/stat` | GET | 单路径元信息 |
| `/api/file/op` | POST | 文件操作（op：rename/copy/cut/recycle/newFile/newDir，批量 ≤128 paths） |
| `/api/reindex` | POST | 触发重建索引 |

（完整请求/响应契约与 4xx 语义以 api-contract.md 为准。）

## 4. Provider 形态伪代码（mnemon 类记忆插件）

记忆 / 知识库插件把 AnyFind 当「事实源」的典型形态：**Provider = 探活 + 查询 + 结果映射
+ 降级**。以 dsh 插件（TypeScript）为例：

```ts
interface AnyfindProvider {
  ready(): Promise<boolean>            // 探活：GET /api/health（超时 ~300ms）
  search(q: string, kinds?: string[]): Promise<Hit[]>  // GET /api/search
}

function createAnyfindProvider(endpoint: string, fetchLike = fetch): AnyfindProvider {
  return {
    async ready() {
      try {
        const r = await withTimeout(fetchLike(`${endpoint}/api/health`), 300)
        return r.ok
      } catch { return false }          // 内核未启动：不算错，返回未就绪
    },
    async search(q, kinds) {
      const url = new URL(`${endpoint}/api/search`)
      url.searchParams.set('q', q)
      if (kinds !== undefined && kinds.length > 0) url.searchParams.set('kinds', kinds.join(','))
      const r = await fetchLike(url)
      if (!r.ok) throw new Error(`anyfind ${r.status}`)
      const { groups } = await r.json()
      // 五组摊平为单数组，组内已按相关性排序；Provider 自行决定截断
      return ['file', 'content', 'session', 'skill', 'memory'].flatMap((k) => groups[k] ?? [])
    },
  }
}

// 记忆插件的调用面（mnemon 类「先搜本地再落新条目」模式）：
const anyfind = createAnyfindProvider('http://127.0.0.1:8790')
async function remember(query: string) {
  if (!(await anyfind.ready())) return { source: 'none', hits: [] }   // 降级：空结果而非报错
  const hits = await anyfind.search(query, ['session', 'memory'])     // 只取会话+记忆两类
  return { source: 'anyfind', hits: hits.slice(0, 8) }                // 每类 limit=20 内自行截断
}
```

设计守则（对齐 dsh-anyfind 插件 `runSearch` 的既有降级语义）：

- **探活与查询分离**：health 失败 → 返回「未就绪」（可附引导文案「启动 AnyFind 桌面版」），
  不要把连接错误抛给上层；
- **kinds 白名单**：记忆类 Provider 通常只要 `session,memory,skill`；不传则五类全给，
  文件类命中会稀释信号；
- **不缓存或只短缓存**：内核自身毫秒级返回 + USN 实时更新，长 TTL 缓存只会拿到陈旧结果；
- **`<mark>` 处理**：snippet 里的高亮标记留给 UI 层，喂模型前先剥掉。

## 5. 能力边界

**能做到的**（数字为 docs/bench-001.md 与 README 实测）：

- **全文检索**：文档内容 FTS + 文件名 MFT/USN 实时索引；172 万文件全盘索引 21.5s，
  冷启动装载名字快照 155ms；
- **拼音**：文件名拼音索引（`anyfind-core/src/scanner.rs:25-34` `pinyin_of`，库表
  `db.rs:79-80` pinyin/pinyin_init 列）——中文文件名可用拼音串搜；
- **六家 harness 会话**：claude-code / codex / dsh / opencode / pi / cursor 的历史会话
  原文（`.jsonl.zstd` / sqlite / jsonl 私有格式适配器），`agent:` 字段可过滤；
- **速度**：击键 → 结果端到端常用词 2-12ms、组合字段 ≤27ms（预算 <50ms，README 实测表）；
  引擎级单项延迟对比（FTS5 trigram vs Tantivy ngram，会话语料）见 docs/bench-001.md；
- **会话可续**：session 命中带 `session_id` + `cwd`，`/api/resume` 直接取回负载。

**做不到 / 注意**：

- **不是向量检索**：DSL 是关键词/短语/字段级，没有语义 embedding——意图改写是调用方
  （LLM）的事；
- **loopback 无认证**：HTTP daemon 只应绑 127.0.0.1，不要暴露到局域网；浏览器端直连有
  CORS 问题，dsh 内走插件同源代理路由（`/dsh-anyfind/api/*`）；
- **读窗口上限**：file/read ≤2MB / 500 行；更大的自取 `path` 直接开文件；
- **索引写者唯一**：同库同时只有一个 anyfindd 做索引（实例锁）；stdio 通道默认 `--no-index`
  只读跟随桌面端索引，桌面端没跑过时库可能是空的；
- **DSL 版本**：v1 语法（见 §3）；不认识的字段前缀按裸词处理。
