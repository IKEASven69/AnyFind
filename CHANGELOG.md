# Changelog

本项目遵循 [语义化版本](https://semver.org/lang/zh-CN/)。dsh 插件子包（`apps/dsh-plugin`，npm 包名 `dsh-anyfind`）独立版本化。

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
