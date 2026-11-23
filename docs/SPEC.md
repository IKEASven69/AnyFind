# AnyFind v1 — 最终规格（SPEC）

> 2026-10-20。取代前身项目 PRD 的两步路线，直接按最终形态构建。
> 产品名称已正式确定为 AnyFind。

## 一句话

Windows 上 Everything 级的即时搜索：一个索引统一 **文件名 / 文档内容 / 六家 AI harness 会话原文 / skills / agent 记忆**；人用 Tauri 桌面壳，agent 用 dsh 插件 + MCP server。

## 架构（最终形态，不再分两步）

```
┌──────────────────────────────────────────────────────┐
│ anyfind-core（Rust lib）                                │
│  MFT/USN 实时索引 │ 内容索引(FTS5或Tantivy,基准选型)      │
│  六家 harness 适配器 │ skills/记忆索引 │ 查询 DSL 解析     │
├──────────────────────────────────────────────────────┤
│ anyfindd（HTTP daemon，实现 api-contract.md）            │
├────────────┬─────────────────┬────────────────────────┤
│ Tauri 桌面壳 │ dsh 插件(已有)   │ MCP server(后续)         │
│ (人,全局热键) │ dsh-anyfind（2026-11-04 已改名，原 dsh-recall）  │                        │
└────────────┴─────────────────┴────────────────────────┘
```

- 参考实现（可移植的 Python 逻辑）：前身项目的 `session_index.py`、`agent_files_index.py`、`server.py`、`scanner.py`、`mft_index.py`（Rust 内核为对应重写）
- API 契约：`docs/api-contract.md`（v0.1，dsh 插件已对接，不得破坏；可扩展）
- UI 资产：前身项目 `desktop.html`（液态玻璃暗色 + 分段控件，Tauri 壳复用并按下文修正）

## 延迟预算（验收硬指标）

| 指标 | 目标 | 测量 |
|---|---|---|
| 文件名击键→结果 | <50ms（对标 Everything <16ms，允许带内容搜索的折衷） | criterion + 实测 |
| 切换类型 tab | **0ms（纯客户端过滤，数据已随搜索一次性返回）** | UI 测试 |
| 冷启动到可搜 | <500ms（索引已建） | 实测 |
| 驻留内存 | <150MB | 实测 |
| 首次全量索引 | <60s（106 万文件本机语料） | 实测 |

## 查询 DSL v1（做深不做大）

- 裸词 AND；`"短语"` 精确；`a OR b`；`-排除`
- 字段：`kind:file|content|session|skill|memory`、`agent:claude-code|codex|dsh|opencode|pi|cursor`、`project:X`、`ext:pdf;docx`、`size:>10mb|1mb..5mb`、`date:today|week|>2026-10-01|2026-10-01..2026-10-20`、`in:<路径前缀>`、`role:user|assistant`（session 专用）
- 允许纯字段查询（如 `kind:session agent:codex`）：按过滤条件浏览，会话/内容/技能/记忆按 mtime 新到旧，文件按索引序；完全空查询仍返回空
- 输入框即解析器：字段 token 染色（字段名一色、值一色）、非法语法实时红波浪提示、`agent:`/`project:`/`in:` 的值从索引实时 suggest 下拉
- 解析器放内核（Rust），UI 与内核共用同一语法定义（避免两套方言）

## UI 修正（对前身桌面版实测问题的硬要求）

1. **类型切换瞬时**：`/api/search` 一次返回全部 groups，切 tab 纯客户端过滤，禁止重新请求；搜索结果随查询缓存，切 tab 零渲染延迟（虚拟列表如需）
2. **加载态**：「搜索中…」居中于整个结果区域（带 skeleton 行），不是输入框下方小字
3. **流式上屏**：文件名组先到先渲染，其余组流式追加（HTTP chunked 或先快后慢两次请求，实现方式自选但体验必须"先看到东西"）
4. 分段控件/图标/角标设计沿用 desktop.html 现版（用户已认可）

## 六家 harness 适配器（Tier 1）

claude-code / codex / dsh（.jsonl.zstd 多版本目录取最高版）/ opencode（storage 三层）/ pi / cursor（探测占位）。逻辑从前身 Python 版移植，proptest 覆盖畸形行容错。各家 resume 命令生成保持契约 /api/resume 语义。每家会话重建在单事务内完成，搜索、原文预览和 resume 不得看到删旧插新之间的空窗。

## 验收（第三方可证）

1. **Everything 对比**：同机同查询集延迟/召回对比表（文件名搜索）
2. **黄金集**：50 条标注「查询→应命中」自动化评测，全过
3. **契约回归**：dsh-anyfind 插件（apps/dsh-plugin）不换内核地址只换端口即可工作——证明 HTTP 契约未破坏
4. **benchmark 报告**：延迟预算表逐项实测数据

## 两路并行分工

- **A 路（本仓 core/）**：anyfind-core + anyfindd。选型实验（rusqlite FTS5 trigram vs Tantivy，真实语料基准后定）、MFT/USN、六家适配器、DSL 解析器、HTTP 契约服务
- **B 路（本仓 apps/desktop/）**：Tauri v2 壳。复用 desktop.html UI 资产并按上文修正三点；自动对接 Rust 内核，普通设置不暴露内核地址；全局热键/托盘/自启动用官方插件
- 接口即 api-contract.md，两路互不等待。release 由 `scripts/build-release.ps1` 构建，anyfindd 作为 externalBin 打进 NSIS；桌面版优先复用 8790，冲突时自动选择空闲端口并写入内部配置，dsh 插件启动时自动发现；自定义远程地址不干预
- 大型索引默认放非系统盘的 `AnyFindData`，小型配置仍放 `%APPDATA%/anyfind/config.json`。桌面设置提供原生目录选择；切换时先复制并校验 `index.db` 与 `names.bin`，新目录内核验证成功后才删除旧索引，不覆盖目标已有文件
- 同一 `index.db` 由系统级互斥锁保证单内核；先绑定 HTTP 端口再启动后台索引。健康接口读内存计数，桌面端在单实例生效后监督内核，异常退出自动恢复，索引迁移期间暂停监督
- MFT 批量写入页缓存上限 128MB，批量结束恢复 32MB 并执行 `PRAGMA shrink_memory`；大文件名内存索引的驻留占用单独优化，不与 SQLite 缓存混算
