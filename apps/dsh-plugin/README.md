# dsh-anyfind

AnyFind 接入 DeepSeek Harness（dsh）：**Ctrl+K 全局搜索** + agent 工具 `anyfind_search` + 斜杠命令 `/anyfind`。

一个索引统一查询：**文件名 / 文档内容 / 六家 AI harness 会话原文 / skills / agent 记忆**。

> 人用 Tauri 桌面壳（Ctrl+K 全局热键），agent 用本插件（`/anyfind` + `anyfind_search`）——同一个内核。

## 为什么

你的会话散落在 claude-code / codex / dsh / opencode / pi / cursor 六家 harness，想找回"上周那个改过 nginx 配置的会话"只能挨家翻。Everything 只索引文件名；全文搜索工具索引不了 AI 会话的私有格式（`.jsonl.zstd` / sqlite / jsonl）。AnyFind 一个内核全部拿下。

## 能做什么

| 入口 | 用法 |
|---|---|
| **斜杠命令** | `/anyfind 配置` 看当前内核地址；`/anyfind <查询>` 直接搜 |
| **agent 工具** | `anyfind_search` —— agent 主动召回历史会话、文件、记忆、skills |
| **Ctrl+K 浮层** | 客户端侧全局搜索面板（装了桌面端或内核后可用） |

查询 DSL（v1）：裸词 AND、`"短语"`、`OR`、`-排除`，以及字段过滤：

```
kind:file|content|session|skill|memory
agent:claude-code|codex|dsh|opencode|pi|cursor
project:  ext:  size:  date:  in:  role:user|assistant
```

例：`nginx 配置 agent:codex date:2026-10`、`kind:session "端口被占" -test`

## 装

```bash
dsh plugin add <path-to-repo>/apps/dsh-plugin
```

内核地址**自动发现**（`%APPDATA%/anyfind/config.json`）；没有桌面端时默认 `127.0.0.1:8790`。内核 down 时工具会返回友好引导而不是抛错。

需要 AnyFind 内核（`core/anyfindd`）在跑——桌面版会自动带起，纯 agent 场景可单独启动 daemon。

## 实测数字

| 指标 | 实测 |
|---|---|
| 172 万文件全盘索引 | **21.5s**（枚举 6.3s + 写库 15.3s，提权 MFT） |
| 击键 → 结果 | 常用词 2-12ms；组合字段 ≤27ms（预算 <50ms） |
| 冷启动装载 172 万名字快照 | 155ms |
| 首次全量会话索引 | ~35s（329 会话 15.7k 消息含 zstd + 1434 skills） |
| 契约回归 | 51 项 0 失败；插件 selftest 28/28 |

复现脚本与基准报告见主仓 `docs/`。

## 环境

- dsh ≥ `0.1.7-rc.1`（peer 为 `*`，实测 0.2.x）
- 内核可选提权：管理员权限启用 MFT/USN 全盘实时索引；未提权自动降级目录遍历 + 周期重扫

## 文档

- 规格：`docs/SPEC.md`
- API 契约：`docs/api-contract.md` v0.1（桌面 / dsh 插件 / 未来 MCP server 共用，扩展不破坏）
- 验收报告：`docs/bench-001.md`、`docs/regression-001.md`

## License

MIT
