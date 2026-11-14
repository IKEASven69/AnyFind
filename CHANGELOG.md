# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。dsh 插件子包（`apps/dsh-plugin`，npm 包名 `dsh-anyfind`）独立版本化。

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
