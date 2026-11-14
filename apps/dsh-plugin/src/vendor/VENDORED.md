# Vendored preview libraries（vendor 目录来源说明）

- 来源仓库：https://github.com/tm5452/dsh-workspace-explorer
- 来源版本：v1.1.32，commit `1bb2f1a20d60faa235583bfe96f5f98a2fc6a6a0`（2026-11-13）
- 许可证：MIT，Copyright (c) 2026 LeiYuXingZhou —— 全文见本目录 `LICENSE-preview-extra`；
  各源文件顶部的版权/署名头随拷贝原样保留。
- 拷贝范围（原样复制，未改逻辑）：
  - `src/vendor/archive/**` —— zip/tar/gz/bz2/xz/zst/7z/rar 列举与解压（纯 ESM，DecompressionStream）
  - `src/vendor/formats/**` —— TOML / Intel HEX / JCAMP-DX / VCD / KiCad / DFU 解析
  - `src/vendor/model/**` —— STL/PLY/OBJ/glTF 解析与 WebGL 渲染
  - `src/vendor/ui/views.js`、`src/vendor/ui/extra-views.js`、`src/vendor/ui/preview-unavailable.js`
    —— 预览正文 React 组件（`previewDefinitions` / `previewBodies` / `previewCss`）
- 唯一改动：`views.js` 与 `extra-views.js` 中
  `import { PreviewUnavailable } from '../../ui/preview-unavailable.js'`
  改为 `'./preview-unavailable.js'`（上游该文件位于其 `src/ui/`，本仓库无对应路径；
  文件已一并 vendor 至 `src/vendor/ui/preview-unavailable.js`）。
- 运行时约定：这批文件使用自由变量 `react`（模块顶层 `const h = react.createElement`），
  由 `scripts/build.mjs` 的 `__ModuleLoader__` 工厂作用域注入 `var react = require("react")`
  （与上游 scripts/build-client.mjs 同一注入方式）；`primitives` 有 `typeof` 守卫，缺失时走纯 DOM 兜底。
- 接入点：`src/client.ts` 预览面板按扩展名命中 `previewDefinitions` 后，
  经 `/dsh-anyfind/api/file/bytes` 取字节并渲染 `previewBodies[id]`。
- pdfjs-dist（Apache-2.0）经 esbuild 内嵌于 lib/client.js；其许可文本随包分发于顶层 LICENSE-pdfjs（终审发现的再分发合规补丁，2026-11-14）。
