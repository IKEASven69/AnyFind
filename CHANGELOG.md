# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。dsh 插件子包（`apps/dsh-plugin`，npm 包名 `dsh-anyfind`）独立版本化。

## [0.3.1] - 2026-11-20

检索底座版本：AnyFind 从「人用的桌面搜索」升级为「agent / 插件可接入的检索底座」——dsh 插件在宿主内注册 MCP server（通道 A），配 Provider 接入指南（`docs/provider-integration.md`）；文件工作台增补六件（dsh 插件端）＋桌面三件。上轮 [未发布] 的 stdio MCP server（通道 B）随本版一并发布。

### Added

- **检索底座化·宿主内 MCP 注册（通道 A）**：dsh-anyfind 插件经宿主 `mcpManager` 服务（`@deepseek-ai/dsh-mcp-manager`，本版新增 optional peer 依赖）注册 stdio server `anyfind`（`anyfindd mcp --no-index`，只读共享索引，写者留给桌面端）——宿主工具面由此挂上三把 anyfind 工具。anyfindd 可执行自动发现（桌面 NSIS sidecar → 开发仓 release 产物，`discoverAnyfindd`）；插件 config 可覆盖：`mcp`（总开关，缺省开）/ `mcpCommand` / `mcpArgs`。**mcpManager 服务缺席（宿主未装 mcp-manager）或可执行全缺时静默跳过并告警**（`ctx.inject` 不触发，与 tools/commands/webServer 同一「服务缺席优雅降级」策略），插件其余功能不受影响
- **Provider 接入指南**（`docs/provider-integration.md`，新文件）：三通道总览（A 宿主内 MCP / B 独立 stdio MCP / C HTTP 直连——同一内核、同一索引库，A/B 可并存）、HTTP 契约速查与 DSL 要点、mnemon 类记忆插件的 Provider 伪代码（探活 + 查询 + 结果映射 + 降级）、能力边界（能做到 / 做不到）
- **MCP server（通道 B）**：`anyfindd mcp [--db <path>] [--no-index]` 子命令——任意 MCP 客户端（Claude Code / Cursor / 其他 harness）经 stdio 用统一搜索，不起 HTTP 监听。协议层手搓 JSON-RPC 2.0（换行分隔，零新依赖，serde_json 已有）：
  - **MCP 子命令**：复用 HTTP 主路径初始化（`ANYFIND_DATA_DIR` 跟随 `--db`、`DbInstanceGuard` 单写者锁、`names.bin` 冷启动装载、连接池），以 stdio 服务循环替代 HTTP 监听；搜索走进程内同一套 `unified_search`。**stdout 只输出协议响应，一切日志/横幅走 stderr**
  - **方法**：`initialize`（`protocolVersion` 回显客户端请求值，缺省 `2025-06-18`；`capabilities:{tools:{}}`；`serverInfo:{name:"anyfind", version:<内核版本>}`）、`notifications/initialized`（无响应）、`ping` → `{}`、`tools/list` → 3 工具、`tools/call` 分派；请求带 id 才回响应，notification（无 id）不回
  - **工具**（返回统一 `{content:[{type:"text", text: JSON.stringify(...)}]}`，`isError` 仅工具级失败时 true）：
    - `anyfind_search{q, kinds?, limit?}` —— 五组命中摊平为单一数组文本；`limit` 钳制 1..50 缺省 20；`kinds` 过滤未知类别（空 = 全部，与 `/api/search` 同语义）
    - `anyfind_file_read{path, lines?}` —— `{content, totalLines, truncated}`；`lines` 钳制 1..500 缺省 30；复用内核 `require_absolute`/`canonicalize_limited`/二进制嗅探（NUL → `binary:true`）/行分页，>2MB 取前 2MB 置 `truncated`
    - `anyfind_file_stat{path}` —— `{path, name, is_dir, size, mtime}` 元信息
  - **错误码**：坏 JSON → `-32700`（id:null 回执）；未知方法 → `-32601`；未知工具 / 坏参数 → `-32602`；工具执行失败（路径不存在等）以 `isError:true` 结果返回而非协议错误
  - **单写者锁策略**：索引（写）模式持 `DbInstanceGuard`；锁被占用（如桌面 anyfindd 在跑）时**降级只读**（不开索引线程，WAL 多进程读安全），MCP server 永不因实例锁拒起；`--no-index` 直接只读
- **内核盘符端点** `GET /api/file/drives`（契约 v0.2 补充，2026-11-21）：Windows 走 `GetLogicalDrivesW` 位掩码 + `GetDriveTypeW`（type 枚举 fixed/removable/cdrom/ram/unknown，不引新依赖），非 Windows 固定返回根 `/`；无参数、永远 200——两端文件浏览器的根导航入口
- **文件工作台六件（dsh 插件端）**：
  - **① 收藏夹**：localStorage 键 `anyfind.favorites` 持久（上限 200、溢出丢最旧）；树 / 结果行星标一键切换；浮层新增「收藏」视图（拖拽排序 / 点击定位 / `storage` 事件跨 tab 共享同一 store）
  - **② 盘符首页**：文件视图「▦」进根级盘符列表（`/api/file/drives`，type 区分图标），点击进各盘根
  - **③ 行内重命名与批量**：F2 / 双击名字原位改名（Esc 取消）；Ctrl / Shift 多选 → 批量工具栏（复制 / 剪切暂存 → 粘贴到当前目录、删除 → 回收站；≤128 上限提示）——0.2.0 已知未完成项「copy/cut 无 UI 入口」在插件端清偿
  - **④ 压缩包浏览**：zip 族（zip/jar/war/apk/epub/cbz/xpi）与 tar/gz/tgz 经 `/api/file/bytes` → vendor archive 解析（Web DecompressionStream），条目树懒展开、文本条目预览、任意条目「引用」/拖拽（`压缩包路径!/条目路径` 格式，按钮与拖拽同源）
  - **⑤ 媒体播放**：音视频 bytes → blob URL 供 `<video>` / `<audio>` 控件（走内核 ≤2MB 字节通路；视频 >2MB 明确降级「用系统应用打开」+提示）
  - **⑥ 划词引用**：文本预览选中行 → `路径:起-止行` + 首行摘要（折叠空白，>160 字符截断）；PDF 文本层选区 → `路径:第 N 页` + 摘要
- **桌面三件（apps/desktop）**：盘符首页（目录树根级「▦」）/ 收藏夹（树与结果行星标 + 侧栏收藏栏拖拽排序，同一 `anyfind.favorites` 键）/ 媒体播放（`<video>` / `<audio>`，>2MB 降级阈值对齐内核字节上限）；标签关闭时释放 Blob URL 防媒体句柄泄漏

### Changed

- dsh 宿主客户端包升级：`0.2.1-rc.5` / `0.2.0-rc.2` 全家 → `0.2.2-rc.2`（cordis `^4.0.5` → `~4.1.0`），并新增 optional peer `@deepseek-ai/dsh-mcp-manager`

### Verification

（2026-11-21 实测，Windows）

- `node apps/dsh-plugin/test/selftest.mjs`：**144 通过 / 0 失败**——含通道 A mock `mcpManager` 注册面（stdio entry / 缺省 args = `mcp --no-index` / 不带 url·headers / `mcp=false` 不注册 / `mcpCommand`·`mcpArgs` 透传 / `discoverAnyfindd` 候选探测）与六件纯函数层（favorites 增删改排 / 压缩包条目行与引用格式 / 划词→行号区间）
- `cargo test --workspace`（core/）：**84 项全绿**（anyfind-core 49 + adapters_malformed 2 + search_filters 3 + anyfindd 30——含 mcp 模块 18 项；新增盘符 2 项：`list_drives` 契约形状 / `sort_dedup_drives` 排序去重）
- `node scripts/desktop-smoke.mjs`：**49 项检查全部 ok，0 失败**（结构锚点新增盘符首页 / 收藏夹 / 媒体播放三组）
- `node scripts/mcp-smoke.mjs`：**16 通过 / 0 失败**（release `anyfindd.exe` 真子进程：三工具 / 错误码 / stdout 纯净 / EOF 退出码 0）
- 实机 UI 走查（Playwright → 本机 dsh 宿主）：**5/5 通过**——盘符首页 7 盘符渲染 / 星标跨 reload 持久 / 现造 test.zip 出压缩包条目树（含 hello.txt）/ 真实 wav 搜索命中出 `<audio>` 控件（blob src）/ 插件加载且经 `/dsh-anyfind/api/*` 同源代理打内核（底档 `.tmp-ui-probe/v03-probe-report.json`）

### Known gaps

- **通道 A（宿主内 MCP 注册）实机验证 blocked**：本机 dsh 宿主未装 `@deepseek-ai/dsh-mcp-manager`——recon 实测设置面板无 MCP 分区、会话工具 picker 无 anyfind MCP 工具（截图 `.tmp-ui-probe/v03-recon*.png`），`ctx.inject(['mcpManager'])` 不触发、注册静默跳过（设计内降级，非缺陷）。已验层次：mock 注册面（selftest）+ 通道 B 真子进程（mcp-smoke 16/16）+ 注册语义对 dsh-mcp-manager `0.2.2-rc.2` 源码逐条核对（provider-integration.md §1）；待宿主装上 mcp-manager 后复验
- **桌面 / 插件不对齐**：行内重命名与批量工具栏、压缩包浏览、划词引用三件为插件端先行，桌面端未同步（桌面本轮只落盘符首页 / 收藏夹 / 媒体播放三件）
- **划词引用无实机 UI 走查**：纯函数层 selftest 覆盖（`selectionToLines` / `formatLineReference` / `formatPageReference`），未在宿主 UI 里实走选区
- **媒体播放实机只测了音频**（真实 wav 命中出 `<audio>`）；视频通路为结构断言（desktop-smoke `<video>` createElement 锚点）+ >2MB 降级设计，未以真实视频文件实测
- **桌面端 copy / cut 仍无 UI 入口**：`/api/file/op` 的 copy/cut 插件端已由批量工具栏清偿，桌面右键菜单未暴露——0.2.0 已知项的余留半边
- 桌面富格式预览正文仍是结构化摘要级、弱于插件端（0.2.0 已记，本轮未变）

## [0.2.0] - 2026-11-12

文件工作台版本：吸收 tm5452/dsh-workspace-explorer 的 MIT 预览库（评估见 `docs/ABSORPTION-PLAN.md`，署名随附于各 vendor 目录），搜索之外补齐文件浏览与轻量管理。桌面端与 dsh 插件端共用同一组内核文件端点。

### Added

- **内核文件端点** `/api/file/{list,read,stat,op}`（HTTP 契约升至 **v0.2**，见 `docs/api-contract.md`）：
  - `file/list` 懒加载目录列举：默认 500 条上限 + 截断标记，噪声目录过滤（`.git` `node_modules` `dist` `build` `coverage` `.idea` `target`）
  - `file/read` 文本分页（窗口钳制 1..500 行，前 8KB 含 NUL 判二进制）+ `format=base64` 取字节通路（整文件 ≤2MB，超限 413，供图片 / PDF 预览）
  - `file/op` 六种写操作：`rename`（不跨目录）/ `copy` / `cut` / `recycle`（回收站，非永久删除）/ `newFile` / `newDir`，批量 ≤128，`newName` 合法性校验，CSRF 门禁（带 `Origin` → 403、非 JSON `Content-Type` → 415）
  - dsh 插件同源代理 `/dsh-anyfind/api/file/{list,read,stat,op}` + 插件自有 `file/bytes`、`office/pdf`（宿主 officeKit 转换，能力缺席 503）
- **文件工作台·桌面端**（`apps/desktop`）：目录树 + 多标签（新建/关闭/切换/拖拽排序）+ 预览（文本分页 30/60/120、图片 base64→data URL、PDF 本地 pdfjs、富格式解析层）+ 右键写操作菜单（重命名 / 复制路径 / 删除→回收站 / 新建文件 / 新建文件夹，破坏性二次确认，批量 ≤128）
- **文件工作台·dsh 插件端**：浮层「文件」视图与 betterSidebar「文件」tab 双路径共享同一 store；预览面板（文本分页 / 图片 ≤1MB data URL 内联 / PDF pdfjs-dist / vendor 富格式 React 正文 / Office 转 PDF）；引用链路——会话头胶囊（`conversation.session.header.utilities` + `inputActions` 桥）、「引用」按钮、`@` 触发源（宿主 `inputTriggers`）、拖拽引用（`application/x-anyfind-file`，目录落框生成 `├─└─│` 文本树 ≤200 行、懒加载深度 4 / 节点 300）；Ctrl 点选批量 ≤128
- **vendor 预览库吸收**（MIT，Copyright (c) 2026 LeiYuXingZhou，`LICENSE-preview-extra` 全文随附）：插件端 `src/vendor/**`（3D 模型 / 压缩包 / TOML / Intel HEX / JCAMP-DX / VCD / KiCad / DFU 解析 + React 预览正文），桌面端 `ui/vendor/preview/**`（解析层 + 自研 `rich.js` 接入层）
- **桌面端内核错误文案透出**：ureq 4xx/5xx 读契约 `{error}` 体展示（如「文件超过 2MB」），不再裸抛状态码

### Fixed

- 插件构建（`scripts/build.mjs`）：client 半 `__ModuleLoader__` 工厂作用域注入 `var react = require("react")`——vendor 预览模块顶层使用自由变量 `react`，缺注入时产物加载即崩；selftest 以工厂求值验证注入生效
- 插件构建（`scripts/build.mjs`）：改名遗留 P0——客户端模块注册 id 硬编码 `dsh-recall` 漏改为 `dsh-anyfind`，产物曾以旧 id 注册；已修复并重建
- `file/op` copy/cut 防自嵌套：target 位于源路径子树内（含相等）时在计划层直接拒绝——独立终审实测 `copy X → X/sub` 会触发 `Copy-Item -Recurse` 沿不断增长的子树无限递归复制直至超时、磁盘残留多层嵌套副本（含单测：子树/相等/大小写归一化三态拒绝，兄弟目录不受影响）
- 再分发合规：pdfjs-dist（Apache-2.0）内嵌于 `lib/client.js`，其许可文本现随包分发（`LICENSE-pdfjs` 入 `files` 白名单，声明见 `src/vendor/VENDORED.md`）

### Changed

- 依赖升级：`@deepseek-ai/dsh-client-ui-conversation`、`@deepseek-ai/dsh-client-ui-input-trigger` → **0.2.1-rc.5**（`@` 触发源所依赖的宿主客户端包，`dsh.client.inject` 同步加入）；`@deepseek-ai/cordis` 4.0.4 → ^4.0.5；新增 `pdfjs-dist` ^6.5.438
- 版本号：`dsh-anyfind` 插件与桌面端（`tauri.conf.json`）0.1.0 → 0.2.0

### Verification

- 插件 selftest **93 项 0 失败**（2026-11-14 实测 `node test/selftest.mjs`：file 端点代理 / base64 字节通路 / file/op 校验 / 引用与文本树 / vendor 工厂求值 / 内核 down 降级；0.1.0 为 28 项）
- 内核 `file_ops` 纯函数单测（六种 op 计划 / 128 上限 / rename 跨目录拒绝）与桌面端 `kernel_error_text` 测试随源码附带
- 既有数字基线不变：契约回归 51 项 0 失败、172 万文件索引 21.5s、击键→结果 2-12ms（`docs/bench-001.md` / `docs/regression-001.md`）

## [0.1.0] - 2026-11-12

首次发布。dsh 插件 `dsh-anyfind` 上线 npm。

### Added

- **`anyfind_search` agent 工具** —— 召回文件 / 文档内容 / 六家 harness 会话原文 / skills / agent 记忆
- **`/anyfind` 斜杠命令** —— `/anyfind <查询>` 直接搜，`/anyfind 配置` 查看内核地址
- **Ctrl+K 全局搜索浮层**（客户端侧）
- **内核代理** —— 转发到 `anyfindd`（默认 `127.0.0.1:8790`，自动读 `%APPDATA%/anyfind/config.json`）
- **查询 DSL v1** —— 裸词 AND / `"短语"` / `OR` / `-排除` + `kind:` `agent:` `project:` `ext:` `size:` `date:` `in:` `role:` 字段过滤
- **内核 down 降级** —— 返回友好错误与引导，不抛栈；斜杠命令同样降级

### Changed

- 产品名对齐：`dsh-recall` → **`dsh-anyfind`**（package.json / `cordis.patch.yml` 的 id+name / 路由 `/dsh-anyfind/api/*` / 工具名 / 斜杠命令 / 内部 slot+tab id 全量对齐）

### Verification

- 插件 selftest **28/28 通过**（工具调用 / 代理路由 / 内核 down 降级）
- 主仓契约回归 **51 项 0 失败**（六家 resume / 边界 / 性能 / 增量）
- 主仓 benchmark：172 万文件索引 21.5s，击键→结果常用词 2-12ms

[0.1.0]: https://github.com/IKEASven69/AnyFind/releases/tag/v0.1.0
[0.2.0]: https://github.com/IKEASven69/AnyFind/releases/tag/v0.2.0
