# AnyFind

**Windows 上的「Everything × AI 记忆」即时搜索。** 一个索引统一：文件名 / 文档内容 / 六家 AI harness 会话原文 / skills / agent 记忆。人用 Tauri 桌面壳（Ctrl+K 全局热键），agent 用 dsh 插件（`/anyfind` 斜杠命令 + `anyfind_search` 工具）。

> Agent skill marketplaces aside, your own work memory is scattered across six AI harnesses. AnyFind indexes all of it — plus every file — and finds anything in under 50ms.

## 为什么

- 你的会话散落在 claude-code / codex / dsh / opencode / pi / cursor 六家 harness，想找回"上周那个改过 nginx 配置的会话"只能挨家翻
- Everything 只索引文件名；全文搜索工具索引不了 AI 会话的私有格式（.jsonl.zstd / sqlite / jsonl）
- AnyFind 一个内核全部拿下：MFT/USN 实时文件索引 + 内容 FTS + 六家 harness 适配器 + skills/记忆索引

## 数字（实测，复现脚本在 docs/）

| 指标 | 实测 |
|---|---|
| 172 万文件全盘索引 | **21.5s**（枚举 6.3s + 写库 15.3s，提权 MFT） |
| 击键 → 结果 | 常用词 2-12ms；组合字段 ≤27ms（预算 <50ms） |
| 冷启动装载 172 万名字快照 | 155ms |
| 首次全量会话索引 | ~35s（329 会话 15.7k 消息含 zstd + 1434 skills） |
| 契约回归 | 51 项 0 失败（六家 resume / 边界 / 性能 / 增量） |

## 组成

```
core/anyfind-core   Rust 内核 lib：MFT/USN 实时索引、FTS、六家适配器、查询 DSL
core/anyfindd       HTTP daemon（docs/api-contract.md，loopback 无认证）
apps/desktop        Tauri 桌面壳：液态玻璃暗色 UI、Ctrl+K、托盘、单实例、开机自启
apps/dsh-plugin     dsh-anyfind 插件：/anyfind 斜杠命令 + anyfind_search 工具 + 内核代理
```

查询 DSL（v1）：裸词 AND、`"短语"`、`OR`、`-排除`、`kind:file|content|session|skill|memory`、`agent:claude-code|codex|dsh|opencode|pi|cursor`、`project:`、`ext:`、`size:`、`date:`、`in:`、`role:user|assistant`。

## 安装

- **桌面（人用）**：Releases 下载安装包（Tauri NSIS）；或源码构建 `scripts/build-release.ps1`
- **dsh 插件（agent 用）**：`dsh plugin add` 指向本仓 `apps/dsh-plugin`；内核地址自动发现（`%APPDATA%/anyfind/config.json`），无桌面端时默认 `127.0.0.1:8790`
- **提权**（可选）：管理员权限启用 MFT/USN 全盘实时索引；未提权自动降级目录遍历 + 周期重扫

## 文档

- 规格：`docs/SPEC.md`（延迟预算/DSL/适配器/验收）
- API 契约：`docs/api-contract.md` v0.1（桌面/dsh 插件/未来 MCP server 共用，扩展不破坏）
- 验收报告：`docs/bench-001.md`（criterion 基准 + 提权实测）、`docs/regression-001.md`（51 项契约回归）

## License

MIT
