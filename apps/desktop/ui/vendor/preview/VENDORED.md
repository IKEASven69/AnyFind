# Vendored preview libraries（桌面端 vendor 目录来源说明）

- 来源仓库：https://github.com/tm5452/dsh-workspace-explorer
- 来源版本：v1.1.32，commit `1bb2f1a20d60faa235583bfe96f5f98a2fc6a6a0`（2026-11-13）；
  `src/vendor` 上游最后一次变更是 2026-11-09（v1.1.0），此后至 v1.1.34 未再变动。
- 许可证：MIT，Copyright (c) 2026 LeiYuXingZhou —— 全文见本目录 `LICENSE-preview-extra`。
- 与 dsh 插件侧（`apps/dsh-plugin/src/vendor/`）同源同版本；本目录只复制了**浏览器可用且不依赖
  React 的解析层**（dsh 侧的 `ui/` React 正文组件未同步到桌面端）：
  - `archive/**` —— zip/tar/gz/bz2/xz/zst/7z/rar 列举与解压（纯 ESM，DecompressionStream）
  - `formats/**` —— TOML / Intel HEX / JCAMP-DX / VCD / KiCad / DFU / glTF / text 解析
  - `model/**` —— STL/PLY/OBJ/glTF 解析与 Canvas2D 渲染（`createModelView` 自带拖拽/缩放/降级）
- 拷贝方式：原样复制（含 CRLF 换行差异），未改任何逻辑；文件内无逐文件许可头，
  署名义务由本目录随附的 `LICENSE-preview-extra` 全文履行。
- **本目录其余文件是 AnyFind 自己的代码（MIT，Copyright (c) 2026 IKEASven69），不属于上游**：
  - `rich.js` —— 桌面端原生 JS 预览接入层（扩展名分发 + 取字节 + 调上述解析器渲染）。
    它是本仓库的新代码，不是上游 `ui/views.js` 的移植；因此桌面端的格式正文是
    结构化摘要级（3D 渲染与压缩包列举除外），弱于 dsh 插件侧的 React 正文。
- 接入点：`apps/desktop/ui/index.html` 的 `openFilePreview`/`renderPreviewState`
  按 `window.RichPreview.extKind(ext)` 命中后取字节并调用 `RichPreview.mount`。
