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

## 文件工作台（0.2.0 新增）

搜索之外，0.2.0 吸收了一套文件浏览与轻量管理能力。桌面端（Tauri 壳）与 dsh 插件端共用同一组内核端点 `/api/file/{list,read,stat,op}`（契约 v0.2，见 `docs/api-contract.md`）；预览库 vendor 自 [tm5452/dsh-workspace-explorer](https://github.com/tm5452/dsh-workspace-explorer)（MIT，署名随附于各 vendor 目录的 `LICENSE-preview-extra` / `VENDORED.md`，吸收评估见 `docs/ABSORPTION-PLAN.md`）。

- **目录树**：懒加载——展开目录时才拉取 `/api/file/list`（默认 500 条上限，截断有提示）；噪声目录默认过滤（`.git` `node_modules` `dist` `build` `coverage` `.idea` `target`，可切换显示）；目录优先、名称排序。dsh 插件端目录树与浮层「文件」视图、betterSidebar「文件」tab 共享同一 store（标签 / 展开 / 选中 / 缓存互通）
- **多标签**：新建 / 关闭 / 切换 / 拖拽排序，纯客户端零请求
- **预览**：
  - 文本分页：默认 30 行，30/60/120 档切换（内核窗口钳制 1..500 行）；前 8KB 含 NUL 判定二进制
  - 图片：`/api/file/read?format=base64` 字节通路（内核整文件 ≤2MB 上限，超限 413）拼 data URL；插件侧 ≤1MB 内联，超限或取不到字节降级「用系统应用打开」
  - PDF：本地 pdfjs 渲染（桌面端自带 `ui/vendor/pdfjs`；插件端 pdfjs-dist 随 bundle 打包）
  - 富格式：3D 模型（STL / PLY / OBJ / glTF）、压缩包（zip / tar / gz / bz2 / xz / zst / 7z / rar）、TOML / Intel HEX / JCAMP-DX / VCD 波形 / KiCad / DFU。插件端用上游 React 正文组件；桌面端为自研原生 JS 接入层，正文是结构化摘要级（3D 渲染与压缩包列举除外），弱于插件端
  - Office（doc/docx/xls/xlsx/ppt/pptx）：仅插件端，经宿主 officeKit 转 PDF 后走 pdfjs；能力缺席（503）或转换失败降级「用系统应用打开」
  - 二进制文件：占位提示，不做文本预览
- **写操作**：树节点 / 结果行右键菜单——重命名、复制路径、删除（移入回收站，非永久删除）、新建文件 / 新建文件夹；破坏性操作二次确认；Ctrl 点选批量 ≤128，超限提示。内核 `/api/file/op` 契约另支持 `copy`（复制到目录）与 `cut`（移动）两种 op，**目前桌面与插件 UI 都没有暴露这两个入口**——已知未完成项
- **引用链路**（dsh 插件端）：会话头胶囊按钮（`conversation.session.header.utilities`）呼出浮层，并把当前会话输入框（`inputActions.setDraft`）桥进模块级 store；结果行「引用」按钮把路径引用追加进输入框，无桥时降级为复制路径；`@` 触发源（宿主 `inputTriggers` 服务，依赖 0.2.1-rc.5 客户端包）搜出路径候选回填 reference；条目可拖拽进输入框（`application/x-anyfind-file` MIME），目录落框生成 `├─└─│` 文本树（≤200 行，懒加载深度 4 / 节点 300 双上限）
- **宿主兼容**：betterSidebar / sidebarRightTabs 服务存在时注册「搜索」（呼出浮层）与「文件」（完整文件树）两个 tab；服务缺席时优雅跳过，不影响插件其余功能

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
