/**
 * dsh-anyfind 浏览器半：Spotlight 式居中搜索浮层。
 * v3 形态（最终拍板）：
 *   - 主形态 = 居中浮层（shell.overlay 机制，与 dsh-hippo MemoryOverlay 同套）：
 *     Ctrl+K 呼出（宿主/已装插件均无此组合占用，已排查），Esc/点遮罩关闭，
 *     上下键导航结果、Enter 执行主动作（会话命中=生成并复制 resume 命令，
 *     文件类=复制路径）；
 *   - better-sidebar「搜索」入口与官方 sidebarRightTabs 直注册路径保留，但点击
 *     改为呼出浮层：占位 tab 挂载即 overlayStore.open() + 提示卡（registerTab
 *     只支持开 tab 不支持自定义动作，占位是干净方案）；
 *   - 浮层内容 = 面板整套（分段控件/瞬时切 tab/居中 loading/token 染色/语法卡/
 *     跳回会话/内核设置），居中 680px（max 90vw），分段控件不再受窄面板限制。
 *   - 「文件」视图（浮层顶部分段切换 + betterSidebar 文件 tab 双路径，共享同一
 *     模块级 store）：懒加载目录树（/dsh-anyfind/api/file/list）、噪声过滤、
 *     截断提示、多标签（新建/关闭/切换/排序纯客户端零请求）。
 *   - 第二步A：预览（file/read 文本分页 30/60/120、binary 占位；vendor 富格式
 *     3D/压缩包/波形/固件/字幕/字体/TOML 等走 /api/file/bytes + src/vendor 预览组件
 *     （见 src/vendor/VENDORED.md）、Office(doc/docx/xls/xlsx/ppt/pptx) 走 /api/office/pdf
 *     （宿主 officeKit 转换）、能力缺席时降级「用系统应用打开（复制路径）」）、
 *     会话头胶囊（inputActions 桥）、@ 触发源（search 出路径候选 → reference 插入）、
 *     引用按钮 + 拖拽（application/x-anyfind-file；目录落框生成 ├─└─│ 文本树）。
 *   - 第二步B：树节点/结果行右键写操作菜单（重命名 / 复制路径 / 删除→回收站 /
 *     新建文件 · 新建文件夹 → POST file/op；破坏性 confirm；Ctrl 点选批量 ≤128
 *     超限提示；条目与顺序对齐桌面端 ctxItemsForPaths）。
 *   - 补丁：图片/PDF 预览改走内核 /api/file/read?format=base64（≤2MB 整文件，
 *     文本与二进制通用）：图片 ≤1MB 拼 data URL 内联、超限/失败降级「打开」，
 *     PDF atob 解字节喂 pdfjs；vendor 富格式仍走 /api/file/bytes。
 * 数据通路：同源 fetch /dsh-anyfind/api/*（host 半代理到内核，避开 CORS）。
 * @module dsh-anyfind/client
 */

import { createElement, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
// 0.2.0-rc.2：一方客户端插件直接收 cordis Context。
import type { Context } from '@deepseek-ai/cordis'
// Type-only: ctx.slots（SlotRegistry 服务）由 ui-renderer 的 cordis Context 合并提供。
import type {} from '@deepseek-ai/dsh-client-ui-renderer/client'
// Type-only: ctx.inputTriggers 服务面与 InputTriggerSource/InputTriggerCandidate
// （inject 数组声明依赖后直取）。
import type { InputTriggerCandidate, InputTriggerSource } from '@deepseek-ai/dsh-client-ui-input-trigger/client'
// PDF 预览：主 API 打进产物；worker 同捆并在主线程注册 globalThis.pdfjsWorker
// （pdf.js 走 fake worker，无需独立 worker 文件 / workerSrc URL）。
import * as pdfjs from 'pdfjs-dist'
import 'pdfjs-dist/build/pdf.worker.min.mjs'
import type { FileEntry, FileListResponse, FileOpRequest, FileReadResponse, Hit, PreviewTreeNode, SearchResponse } from './core.ts'
import {
  FILE_OP_MAX_PATHS, KIND_LABEL, KINDS, buildTreeText, escapeWithMark, fmtMtime, formatReference, stripMark,
  validateFileOpRequest,
} from './core.ts'
// 富格式预览正文（vendor 自 tm5452/dsh-workspace-explorer，MIT，见 src/vendor/VENDORED.md）：
// previewDefinitions 提供扩展名 → id，previewBodies 提供 id → React 正文组件。
import { previewBodies, previewDefinitions } from './vendor/ui/views.js'

// inputTriggers：@ 触发源依赖（宿主 0.2.1-rc.5 自带其客户端包，见 package.json inject）。
export const inject = ['slots', 'inputTriggers']

// ---------------------------------------------------------------------------
// 官方侧边栏扩展面结构子集（与 dsh-hippo 同一注册表契约，免依赖 sidebar-right 包）
// ---------------------------------------------------------------------------

interface SidebarRightTabsRegistry {
  register(definition: {
    id: string
    kind: string
    keepMounted?: boolean
    title: () => string
    guide?: readonly { id: string; order: number; title: () => string; description?: () => string; icon?: unknown }[]
  }): () => void
}

interface DynamicSlots {
  inject(key: string, callback: () => () => void): () => void
  register(options: Record<string, unknown>, component: unknown): () => void
}

interface BetterSidebarLike {
  registerTab(descriptor: {
    id: string
    title: () => string
    description?: () => string
    icon?: (size: number) => unknown
    order?: number
    single?: boolean
    component: (props: unknown) => unknown
  }): () => void
  /** v0.12+：按 id 关 tab（占位 tab 即开即合用；未知 id 为 no-op）。 */
  closeTab?(tabId: string, scope?: unknown): void
}

interface SidebarStateLike {
  bottomOpen?: boolean
  bottomSplits?: unknown
  [key: string]: unknown
}

/** better-sidebar TabComponentProps 结构子集（占位 tab 自移除要 tab.id + scope + store）。 */
interface TabComponentPropsLike {
  tab?: { id: string }
  scope?: { sessionId: string; cwd?: string }
  store?: {
    reduce(reducer: (state: SidebarStateLike) => SidebarStateLike): void
    getSnapshot(): { state?: SidebarStateLike }
  }
}

/** 数 bottomSplits 树里剩余 tab 数（防御性遍历，结构不符按 1 处理=不收合）。 */
function countRemainingTabs(node: unknown): number {
  if (node === null || typeof node !== 'object') return 1
  const n = node as { tabs?: unknown; children?: unknown; left?: unknown; right?: unknown }
  if (Array.isArray(n.tabs)) return n.tabs.length
  const kids = ([n.children, n.left, n.right].filter(Array.isArray) as unknown[][]).flat()
  if (kids.length === 0) return 1
  return kids.reduce<number>((sum, k) => sum + countRemainingTabs(k), 0)
}

let betterSidebarSvc: BetterSidebarLike | null = null
let disposeDirectTab: (() => void) | null = null

// ---------------------------------------------------------------------------
// 浮层开关态（dsh-hippo overlayStore 同型：模块级开关 + useSyncExternalStore）
// ---------------------------------------------------------------------------

const overlayStore = {
  open: false,
  /** 浮层内视图：搜索（默认）/ 文件；模块级持久，关掉重开不丢。 */
  view: 'search' as 'search' | 'files',
  listeners: new Set<() => void>(),
  toggle(): void {
    overlayStore.open = !overlayStore.open
    for (const l of overlayStore.listeners) l()
  },
  show(): void {
    if (overlayStore.open) return
    overlayStore.open = true
    for (const l of overlayStore.listeners) l()
  },
  close(): void {
    if (!overlayStore.open) return
    overlayStore.open = false
    for (const l of overlayStore.listeners) l()
  },
  setView(view: 'search' | 'files'): void {
    if (overlayStore.view === view) return
    overlayStore.view = view
    for (const l of overlayStore.listeners) l()
  },
  subscribe(fn: () => void): () => void {
    overlayStore.listeners.add(fn)
    return () => { overlayStore.listeners.delete(fn) }
  },
}

// ---------------------------------------------------------------------------
// 全局 toast：面板内动作与 document 级拖拽共用（拖拽落点可能在任一面板外）
// ---------------------------------------------------------------------------

const toastStore = {
  message: '',
  timer: null as ReturnType<typeof setTimeout> | null,
  listeners: new Set<() => void>(),
  show(message: string): void {
    toastStore.message = message
    if (toastStore.timer !== null) clearTimeout(toastStore.timer)
    toastStore.timer = setTimeout(() => {
      toastStore.message = ''
      toastStore.timer = null
      for (const l of toastStore.listeners) l()
    }, 2400)
    for (const l of toastStore.listeners) l()
  },
  subscribe(fn: () => void): () => void {
    toastStore.listeners.add(fn)
    return () => { toastStore.listeners.delete(fn) }
  },
  get(): string {
    return toastStore.message
  },
}

// ---------------------------------------------------------------------------
// 输入框桥：会话头胶囊把 props.inputActions（跟随当前会话）存进模块级 store，
// 「引用」按钮与拖拽落框共用；store 空（无会话/未挂载）→ 调用方降级复制路径。
// 追加语义与 dsh-workspace-explorer 的 DockBridge 同型：读当前 draft 末尾补换行。
// ---------------------------------------------------------------------------

interface DraftBridge {
  insert(text: string): void
}

let draftBridge: DraftBridge | null = null

function setDraftBridge(bridge: DraftBridge): void {
  draftBridge = bridge
}

function clearDraftBridge(bridge: DraftBridge): void {
  if (draftBridge === bridge) draftBridge = null
}

/** 引用/拖拽落框共用：有桥 → setDraft 追加；无桥 → 复制 + toast（调用方已给文案）。 */
function insertReferenceText(text: string): boolean {
  if (draftBridge === null || text === '') return false
  draftBridge.insert(text)
  return true
}

// ---------------------------------------------------------------------------
// 样式：颜色优先继承宿主设计令牌（--dsw-alias-*，亮暗自动跟随）；
// 分段控件/语法卡/token 染色移植 desktop.html 的设计语言，暗色玻璃值改为令牌派生。
// ---------------------------------------------------------------------------

const CSS = `
.rc-panel { display: flex; flex-direction: column; width: 100%; height: 100%; min-height: 0;
  background: var(--dsw-alias-bg-base, #fff); color: var(--dsw-alias-label-primary, inherit);
  --rc-a: var(--dsw-alias-brand-primary, var(--accent, #2563eb));
  --rc-line: var(--dsw-alias-border-l2, var(--border, rgba(127,127,127,.28)));
  --rc-mut: var(--dsw-alias-label-secondary, var(--muted, rgba(127,127,127,.92)));
  --rc-card: var(--dsw-alias-bg-layer-1, var(--bg, rgba(127,127,127,.05)));
  --rc-err: var(--dsw-alias-state-error-primary, #d93025);
  --rc-ok: var(--dsw-alias-state-success-primary, #15803d);
  --rc-f-name: #6ea8ff; --rc-f-val: #2dd4bf; --rc-f-phrase: #fbbf24;
  --rc-f-excl: #f87171; --rc-f-op: #a78bfa; }
.rc-head { display: flex; gap: 8px; padding: 14px 14px 10px; flex: none; position: relative; z-index: 20; }

/* 搜索框：透明 input 叠 token 染色背板（桌面版 DSL 染色思路的 React 移植） */
.rc-input-wrap { position: relative; flex: 1; min-width: 0; background: var(--rc-card); border-radius: 10px; }
.rc-input { width: 100%; box-sizing: border-box; font-size: 14px; line-height: 22px; padding: 9px 36px 9px 14px;
  border-radius: 10px; border: 1px solid var(--rc-line); background: transparent; outline: none;
  position: relative; z-index: 1; color: transparent; caret-color: var(--dsw-alias-label-primary, #888);
  font-family: inherit; letter-spacing: normal; }
.rc-input::placeholder { color: var(--rc-mut); }
.rc-input:focus { border-color: var(--rc-a); }
.rc-input-hl { position: absolute; inset: 1px; padding: 9px 36px 9px 14px; font-size: 14px; line-height: 22px;
  font-family: inherit; letter-spacing: normal; white-space: pre; overflow: hidden; pointer-events: none;
  color: var(--dsw-alias-label-primary, inherit); z-index: 0; }
.rc-tk-name { color: var(--rc-f-name); font-weight: 600; }
.rc-tk-val { color: var(--rc-f-val); }
.rc-tk-phrase { color: var(--rc-f-phrase); }
.rc-tk-excl { color: var(--rc-f-excl); text-decoration: line-through; text-decoration-thickness: 1px; }
.rc-tk-op { color: var(--rc-f-op); font-weight: 700; }
.rc-syn-btn { position: absolute; right: 8px; top: 50%; transform: translateY(-50%); z-index: 2;
  width: 24px; height: 24px; border-radius: 7px; border: none; cursor: pointer;
  background: transparent; color: var(--rc-mut); font-size: 13px; font-weight: 600; }
.rc-syn-btn:hover { background: rgba(127,127,127,.15); color: var(--dsw-alias-label-primary, inherit); }

.rc-btn { cursor: pointer; border-radius: 9px; font-size: 12.5px; padding: 7px 14px; white-space: nowrap;
  border: 1px solid var(--rc-line); background: transparent; color: inherit; }
.rc-btn:hover { border-color: var(--rc-a); color: var(--rc-a); }
.rc-btn-pri { background: var(--rc-a); border-color: transparent; color: #fff; }
.rc-btn-pri:hover { color: #fff; opacity: .92; }
.rc-btn:disabled { opacity: .5; cursor: default; }

/* kind 分段控件（desktop.html 移植：玻璃容器 + 滑动指示器 + 图标 + 计数角标；超宽横向滚动兜底） */
.rc-seg-scroll { overflow-x: auto; scrollbar-width: none; flex: none; padding: 0 14px 10px; }
.rc-seg-scroll::-webkit-scrollbar { display: none; }
.rc-seg { --rc-accent: #9db4e8; position: relative; display: flex; width: max-content; min-width: 100%;
  box-sizing: border-box; padding: 3px; border-radius: 12px;
  background: var(--dsw-alias-bg-layer-1, rgba(127,127,127,.06)); border: 1px solid var(--rc-line);
  box-shadow: inset 0 1px 0 rgba(255,255,255,.05), inset 0 -2px 4px rgba(0,0,0,.06); }
.rc-seg-ind { position: absolute; top: 3px; bottom: 3px; left: 3px; width: 0; border-radius: 9px;
  background: color-mix(in srgb, var(--rc-accent) 24%, transparent);
  box-shadow: 0 0 12px color-mix(in srgb, var(--rc-accent) 28%, transparent),
    inset 0 0 0 1px color-mix(in srgb, var(--rc-accent) 45%, transparent);
  transition: left .28s cubic-bezier(.32,.72,.28,1), width .28s cubic-bezier(.32,.72,.28,1), background .25s, box-shadow .25s;
  pointer-events: none; }
.rc-seg-btn { --sa: #9db4e8; position: relative; z-index: 1; flex: 1 0 auto; display: flex; align-items: center;
  justify-content: center; gap: 5px; padding: 5px 10px; font-size: 12px; font-weight: 500; font-family: inherit;
  letter-spacing: .2px; background: transparent; border: none; border-radius: 9px; cursor: pointer;
  color: var(--rc-mut); transition: color .18s; white-space: nowrap; }
.rc-seg-btn:hover { color: var(--dsw-alias-label-primary, inherit); }
.rc-seg-btn.active { color: var(--dsw-alias-label-primary, inherit); font-weight: 600; }
.rc-seg-btn.active .rc-seg-ico { color: var(--sa); }
.rc-seg-ico { width: 13px; height: 13px; flex-shrink: 0; }
.rc-seg-count { font-size: 9px; font-variant-numeric: tabular-nums; letter-spacing: .5px; line-height: 1;
  color: var(--rc-mut); background: rgba(127,127,127,.12); padding: 2.5px 5px; border-radius: 7px; }
.rc-seg-btn.active .rc-seg-count { color: var(--sa); background: rgba(0,0,0,.18); }
.rc-seg-count:empty { display: none; }

.rc-body { flex: 1; overflow-y: auto; padding: 4px 14px 14px; display: flex; flex-direction: column; gap: 10px;
  position: relative; }

/* 加载态：结果区居中 + skeleton 占位行 */
.rc-loading { display: flex; flex-direction: column; align-items: center; justify-content: center;
  gap: 14px; flex: 1; min-height: 160px; width: 100%; }
.rc-loading-t { font-size: 12.5px; color: var(--rc-mut); display: flex; align-items: center; gap: 8px; }
.rc-spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid rgba(127,127,127,.3);
  border-top-color: var(--rc-a); animation: rcSpin .7s linear infinite; }
@keyframes rcSpin { to { transform: rotate(360deg); } }
.rc-skel { width: 100%; max-width: 520px; display: flex; flex-direction: column; gap: 10px; }
.rc-skel-row { height: 34px; border-radius: 9px; background: var(--rc-card); border: 1px solid var(--rc-line);
  position: relative; overflow: hidden; }
.rc-skel-row::after { content: ''; position: absolute; inset: 0;
  background: linear-gradient(90deg, transparent, rgba(127,127,127,.16), transparent);
  animation: rcShimmer 1.3s ease infinite; }
.rc-skel-row:nth-child(2) { width: 88%; } .rc-skel-row:nth-child(3) { width: 72%; }
@keyframes rcShimmer { from { transform: translateX(-100%); } to { transform: translateX(100%); } }

/* 语法卡弹层 */
.rc-syn { position: absolute; top: calc(100% + 2px); right: 14px; z-index: 40; width: min(380px, calc(100% - 28px));
  border-radius: 12px; padding: 12px 14px; background: var(--dsw-alias-bg-layer-2, var(--dsw-alias-bg-base, #fff));
  border: 1px solid var(--rc-line); box-shadow: 0 12px 40px rgba(0,0,0,.28);
  display: flex; flex-direction: column; gap: 2px; animation: rcIn .18s ease both; }
.rc-syn-t { font-size: 12px; font-weight: 700; margin-bottom: 6px; }
.rc-syn-row { display: flex; gap: 10px; align-items: baseline; padding: 3px 4px; border-radius: 7px; cursor: pointer; }
.rc-syn-row:hover { background: rgba(127,127,127,.1); }
.rc-syn-row code { font-size: 11.5px; font-weight: 700; color: var(--rc-f-name); flex: none; min-width: 74px; }
.rc-syn-row span { font-size: 11px; color: var(--rc-mut); }
.rc-syn-ex { font-size: 11px; color: var(--rc-mut); margin-top: 8px; border-top: 1px solid var(--rc-line); padding-top: 8px; }
.rc-syn-ex code { color: var(--rc-f-val); user-select: all; }

.rc-group { display: flex; flex-direction: column; gap: 6px; }
.rc-group-t { font-size: 12px; font-weight: 700; color: var(--rc-mut); display: flex; gap: 6px; align-items: center; }
.rc-hit { border: 1px solid var(--rc-line); border-radius: 10px; padding: 9px 12px; background: var(--rc-card);
  display: flex; flex-direction: column; gap: 4px; animation: rcIn .25s ease both; }
.rc-hit.focused { border-color: var(--rc-a); box-shadow: 0 0 0 1px color-mix(in srgb, var(--rc-a) 55%, transparent); }
@keyframes rcIn { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: none; } }
.rc-hit-t { font-size: 12.5px; font-weight: 600; word-break: break-all; }
.rc-hit-meta { font-size: 10.5px; color: var(--rc-mut); display: flex; gap: 8px; flex-wrap: wrap; }
.rc-hit-s { font-size: 12px; line-height: 1.55; color: var(--rc-mut); word-break: break-word; }
.rc-hit-s mark { background: rgba(250, 204, 21, .35); color: inherit; border-radius: 3px; padding: 0 1px; }
.rc-hit-path { font-size: 10.5px; color: var(--rc-mut); word-break: break-all; font-family: monospace; }
.rc-resume { align-self: flex-start; font-size: 11px; padding: 3px 10px; }
.rc-banner { font-size: 12px; line-height: 1.65; border-radius: 9px; padding: 9px 12px; }
.rc-banner-err { color: var(--rc-err); background: rgba(211,47,47,.08); }
.rc-banner-info { color: var(--rc-mut); background: rgba(127,127,127,.08); }
.rc-guide { display: flex; flex-direction: column; gap: 12px; padding: 4px 2px; }
.rc-guide-t { font-size: 14px; font-weight: 700; }
.rc-cmd { font-family: monospace; font-size: 11.5px; background: rgba(0,0,0,.08); border-radius: 8px;
  padding: 8px 10px; word-break: break-all; user-select: all; white-space: pre-wrap; }
.rc-foot { flex: none; border-top: 1px solid var(--rc-line); padding: 8px 12px; display: flex; gap: 8px; align-items: center; }
.rc-foot input { flex: 1; min-width: 0; font-size: 11px; padding: 4px 8px; border-radius: 7px;
  border: 1px solid var(--rc-line); background: transparent; color: inherit; outline: none; font-family: monospace; }
.rc-foot .rc-btn { font-size: 11px; padding: 4px 10px; }
.rc-meta-pill { font-size: 10.5px; color: var(--rc-ok); font-weight: 600; }
.rc-kbd { font-size: 10px; color: var(--rc-mut); border: 1px solid var(--rc-line); border-radius: 5px;
  padding: 1px 6px; font-family: inherit; }
.rc-toast { position: absolute; left: 50%; bottom: 52px; transform: translateX(-50%); z-index: 50;
  font-size: 11.5px; padding: 5px 14px; border-radius: 999px; color: #fff; background: rgba(30,32,40,.92);
  box-shadow: 0 6px 20px rgba(0,0,0,.3); animation: rcIn .18s ease both; pointer-events: none; }

/* Spotlight 浮层：遮罩 blur + 居中玻璃框 */
.rc-mask { position: fixed; inset: 0; z-index: 90; background: rgba(8,10,16,.46);
  backdrop-filter: blur(10px) saturate(140%); -webkit-backdrop-filter: blur(10px) saturate(140%);
  display: flex; align-items: flex-start; justify-content: center; padding-top: 11vh;
  animation: rcFade .16s ease both; }
.rc-modal { width: 680px; max-width: 90vw; height: min(72vh, 660px); display: flex; flex-direction: column;
  border-radius: 16px; overflow: hidden; background: var(--dsw-alias-bg-base, #fff);
  border: 1px solid var(--rc-line); box-shadow: 0 24px 80px rgba(0,0,0,.42), inset 0 1px 0 rgba(255,255,255,.06);
  animation: rcPop .18s cubic-bezier(.3,1.15,.4,1) both; }
@keyframes rcFade { from { opacity: 0; } to { opacity: 1; } }
@keyframes rcPop { from { opacity: 0; transform: translateY(-10px) scale(.98); } to { opacity: 1; transform: none; } }

/* 占位 tab（侧边卡片入口点击 → 呼出浮层 + 提示） */
.rc-ph { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 14px;
  padding: 40px 24px; text-align: center; color: var(--rc-mut); }
.rc-ph-t { font-size: 14px; font-weight: 700; color: var(--dsw-alias-label-primary, inherit); }
.rc-ph kbd { font-size: 11px; border: 1px solid var(--rc-line); border-bottom-width: 2px; border-radius: 6px;
  padding: 2px 8px; background: var(--rc-card); color: var(--dsw-alias-label-primary, inherit); font-family: inherit; }

/* 文件树面板：浮层「文件」视图与 betterSidebar 文件 tab 共用 */
.rc-ovsw { display: flex; gap: 4px; flex: none; padding: 10px 14px 0; }
.rc-ovsw-b { flex: 1; font-size: 12px; font-weight: 600; font-family: inherit; padding: 6px 0; cursor: pointer;
  border: 1px solid var(--rc-line); background: transparent; color: var(--rc-mut); border-radius: 9px; }
.rc-ovsw-b:hover { color: var(--dsw-alias-label-primary, inherit); border-color: var(--rc-a); }
.rc-ovsw-b.active { background: var(--rc-card); color: var(--dsw-alias-label-primary, inherit); border-color: var(--rc-a); }
.rc-ftabs { display: flex; align-items: center; gap: 6px; flex: none; padding: 12px 14px 8px;
  overflow-x: auto; scrollbar-width: none; }
.rc-ftabs::-webkit-scrollbar { display: none; }
.rc-ftab { display: flex; align-items: center; gap: 2px; flex: none; max-width: 190px; padding: 4px 6px 4px 10px;
  border-radius: 9px; border: 1px solid var(--rc-line); background: var(--rc-card); cursor: pointer;
  font-size: 12px; color: var(--rc-mut); }
.rc-ftab.active { border-color: var(--rc-a); color: var(--dsw-alias-label-primary, inherit); font-weight: 600; }
.rc-ftab-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.rc-ftab-ops { display: inline-flex; opacity: 0; transition: opacity .15s; }
.rc-ftab:hover .rc-ftab-ops, .rc-ftab.active .rc-ftab-ops { opacity: 1; }
.rc-ftab-ops button { width: 16px; height: 16px; border: none; background: transparent; color: inherit; cursor: pointer;
  font-size: 12px; line-height: 1; padding: 0; border-radius: 4px; font-family: inherit; }
.rc-ftab-ops button:hover { background: rgba(127,127,127,.18); }
.rc-ftab-ops button:disabled { opacity: .3; cursor: default; background: transparent; }
.rc-ftab-new { flex: none; width: 24px; height: 24px; border-radius: 8px; border: 1px dashed var(--rc-line);
  background: transparent; color: var(--rc-mut); cursor: pointer; font-size: 14px; line-height: 1; }
.rc-ftab-new:hover { border-color: var(--rc-a); color: var(--rc-a); }
.rc-fbar { display: flex; gap: 6px; align-items: center; flex: none; padding: 0 14px 8px; }
.rc-fpath { flex: 1; min-width: 0; font-size: 11.5px; font-family: monospace; padding: 5px 8px; border-radius: 7px;
  border: 1px solid var(--rc-line); background: var(--rc-card); color: inherit; outline: none; }
.rc-fpath:focus { border-color: var(--rc-a); }
.rc-fbar .rc-btn { padding: 5px 10px; }
.rc-tree { flex: 1; overflow-y: auto; padding: 2px 10px 10px; display: flex; flex-direction: column; gap: 1px;
  min-height: 0; }
.rc-main { display: flex; flex-direction: column; flex: 1; min-height: 0; }
.rc-frow { display: flex; align-items: center; gap: 7px; padding: 4px 8px; border-radius: 7px; cursor: pointer;
  font-size: 12.5px; user-select: none; }
.rc-frow:hover { background: rgba(127,127,127,.1); }
.rc-frow.sel { background: color-mix(in srgb, var(--rc-a) 18%, transparent); }
.rc-frow.muted { cursor: default; color: var(--rc-mut); font-size: 11.5px; }
.rc-frow.muted:hover { background: transparent; }
.rc-fico { width: 14px; flex: none; text-align: center; color: var(--rc-mut); font-size: 10px; }
.rc-frow.dir .rc-fico { color: var(--rc-a); }
.rc-fname { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.rc-fmeta { flex: none; font-size: 10.5px; color: var(--rc-mut); font-variant-numeric: tabular-nums; }
.rc-fmeta.w { min-width: 58px; text-align: right; }
.rc-fmeta.d { min-width: 96px; text-align: right; }
.rc-fhint { font-size: 11.5px; line-height: 1.6; color: var(--rc-mut); padding: 6px 8px; }
.rc-fempty { display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 40px 16px;
  text-align: center; color: var(--rc-mut); font-size: 12.5px; }
.rc-ffoot { flex: none; border-top: 1px solid var(--rc-line); padding: 7px 12px; display: flex; gap: 8px;
  align-items: center; }
.rc-fsel { flex: 1; min-width: 0; font-size: 11px; font-family: monospace; overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; color: var(--rc-mut); }

/* 会话头胶囊（conversation.session.header.utilities） */
.rc-cap { display: inline-flex; align-items: center; gap: 5px; cursor: pointer; font-family: inherit;
  font-size: 11.5px; font-weight: 600; line-height: 1; padding: 5px 10px; border-radius: 999px;
  border: 1px solid var(--rc-line); background: var(--rc-card); color: var(--rc-mut); }
.rc-cap:hover { border-color: var(--rc-a); color: var(--rc-a); }
.rc-cap.on { border-color: var(--rc-a); color: var(--dsw-alias-label-primary, inherit);
  background: color-mix(in srgb, var(--rc-a) 14%, transparent); }

/* 行内动作（悬停显现）：预览 / 引用 / 拖拽把手 */
.rc-frow { position: relative; }
.rc-fops { position: absolute; right: 6px; top: 50%; transform: translateY(-50%); display: none; gap: 4px;
  padding-left: 10px; background: linear-gradient(90deg, transparent, var(--rc-card) 18%); }
.rc-frow:hover .rc-fops { display: inline-flex; }
.rc-frow:hover .rc-fmeta { visibility: hidden; }
.rc-fops button, .rc-hit-ops button { font-family: inherit; font-size: 10.5px; line-height: 1; padding: 3px 7px;
  border-radius: 6px; border: 1px solid var(--rc-line); background: var(--dsw-alias-bg-base, #fff);
  color: var(--rc-mut); cursor: pointer; white-space: nowrap; }
.rc-fops button:hover, .rc-hit-ops button:hover { border-color: var(--rc-a); color: var(--rc-a); }
.rc-hit-ops { display: flex; gap: 6px; flex-wrap: wrap; }
.rc-frow[draggable='true'] { cursor: grab; }
.rc-frow[draggable='true']:active { cursor: grabbing; }

/* 预览面板（文本分页 / binary 占位 / 图片降级 / PDF canvas） */
.rc-prev { display: flex; flex-direction: column; gap: 8px; flex: 1; min-height: 0; padding: 2px 10px 10px; }
.rc-prev-head { display: flex; align-items: center; gap: 8px; }
.rc-prev-t { flex: 1; min-width: 0; font-size: 12.5px; font-weight: 600; overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; }
.rc-prev-ctl { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.rc-psz { display: inline-flex; gap: 0; border: 1px solid var(--rc-line); border-radius: 8px; overflow: hidden; }
.rc-psz button { font-family: inherit; font-size: 10.5px; padding: 3px 8px; border: none; cursor: pointer;
  background: transparent; color: var(--rc-mut); }
.rc-psz button + button { border-left: 1px solid var(--rc-line); }
.rc-psz button.on { background: color-mix(in srgb, var(--rc-a) 18%, transparent); color: var(--rc-a); font-weight: 700; }
.rc-pvinfo { font-size: 10.5px; color: var(--rc-mut); font-variant-numeric: tabular-nums; }
.rc-pvbody { flex: 1; min-height: 0; overflow: auto; border: 1px solid var(--rc-line); border-radius: 9px;
  background: var(--rc-card); padding: 8px 10px; }
.rc-pvrich { padding: 0; display: flex; flex-direction: column; }
.rc-pvrich > * { flex: 1; min-height: 0; width: 100%; }
.rc-pvimg { display: flex; align-items: center; justify-content: center; padding: 4px; }
.rc-pvimg img { max-width: 100%; max-height: 100%; object-fit: contain; }
.rc-pvpre { margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 11.5px;
  line-height: 1.55; white-space: pre-wrap; word-break: break-all; color: inherit; }
.rc-pvph { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px;
  min-height: 160px; text-align: center; color: var(--rc-mut); font-size: 12px; padding: 16px; }
.rc-pvcanvas { max-width: 100%; height: auto; display: block; margin: 0 auto; background: #fff; border-radius: 4px; }
.rc-pvrows { display: flex; flex-direction: column; gap: 2px; }
.rc-pvrow { display: flex; gap: 8px; align-items: baseline; font-size: 11.5px; padding: 2px 4px; border-radius: 6px; }
.rc-pvrow:hover { background: rgba(127,127,127,.1); }
.rc-pvrow .n { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.rc-pvrow .m { flex: none; font-size: 10px; color: var(--rc-mut); font-variant-numeric: tabular-nums; }

/* 右键写操作菜单（树节点 / 搜索结果行） */
.rc-ctx { position: fixed; z-index: 120; min-width: 196px; max-width: 320px; padding: 5px;
  border-radius: 11px; background: var(--dsw-alias-bg-layer-2, var(--dsw-alias-bg-base, #fff));
  border: 1px solid var(--rc-line); box-shadow: 0 16px 44px rgba(0,0,0,.32);
  display: flex; flex-direction: column; gap: 1px; animation: rcIn .12s ease both; }
.rc-ctx-head { font-size: 10.5px; color: var(--rc-mut); padding: 4px 10px 6px; overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; }
.rc-ctx button { display: flex; align-items: center; gap: 8px; width: 100%; text-align: left;
  font-family: inherit; font-size: 12.5px; padding: 7px 10px; border: none; background: transparent;
  color: inherit; border-radius: 7px; cursor: pointer; }
.rc-ctx button:hover:not(:disabled) { background: rgba(127,127,127,.14); }
.rc-ctx button:disabled { opacity: .4; cursor: default; }
.rc-ctx button.danger { color: var(--rc-err); }
.rc-ctx button.danger:hover { background: rgba(211,47,47,.12); }
.rc-ctx-sep { height: 1px; background: var(--rc-line); margin: 4px 6px; }

@media (prefers-reduced-motion: reduce) { .rc-hit, .rc-syn, .rc-mask, .rc-modal, .rc-toast, .rc-ctx { animation: none !important; }
  .rc-seg-ind { transition: none !important; } .rc-skel-row::after, .rc-spin { animation: none !important; } }
`

function StyleTag(): ReturnType<typeof createElement> {
  return createElement('style', null, CSS)
}

// ---------------------------------------------------------------------------
// 数据获取（同源代理；down 标记 = 内核未启动）
// ---------------------------------------------------------------------------

class CoreDown extends Error {
  /** down 响应里宿主带回的当前配置地址（引导页/地址栏展示用）。 */
  endpoint?: string
}

interface HealthState {
  ok: boolean
  version?: string
  index?: { files?: number; sessions?: number; skills?: number; memories?: number }
  endpoint?: string
}

/** down 响应构 CoreDown（带上宿主配置的 endpoint）。 */
function coreDown(body: { error?: string; endpoint?: string }, fallback: string): CoreDown {
  const err = new CoreDown(body.error ?? fallback)
  if (typeof body.endpoint === 'string') err.endpoint = body.endpoint
  return err
}

async function getHealth(): Promise<HealthState> {
  const res = await fetch('/dsh-anyfind/api/health', { cache: 'no-store' })
  const body = await res.json() as HealthState & { down?: boolean; error?: string }
  if (!res.ok || body.down === true) throw coreDown(body, `HTTP ${res.status}`)
  return body
}

/** 一次拉全量 groups（不带 kinds）——kind 切换纯客户端过滤，零请求。 */
async function getSearch(q: string): Promise<SearchResponse> {
  const url = new URLSearchParams({ q, limit: '10' })
  const res = await fetch(`/dsh-anyfind/api/search?${url.toString()}`, { cache: 'no-store' })
  const body = await res.json() as SearchResponse & { down?: boolean; error?: string; endpoint?: string }
  if (body.down === true) throw coreDown(body, '内核未启动')
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`)
  return body
}

async function postResume(hit: Hit): Promise<string> {
  const res = await fetch('/dsh-anyfind/api/resume', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: hit.agent, session_id: hit.session_id, cwd: hit.cwd }),
  })
  const body = await res.json() as { ok: boolean; command?: string; error?: string; down?: boolean; endpoint?: string }
  if (body.down === true) throw coreDown(body, '内核未启动')
  if (!res.ok || !body.ok || typeof body.command !== 'string') throw new Error(body.error ?? `HTTP ${res.status}`)
  return body.command
}

/** 目录列表（契约 v0.2 /api/file/list）：400/404/502 都带内核 error 文案给面板。 */
async function getFileList(path: string): Promise<FileListResponse> {
  const res = await fetch(`/dsh-anyfind/api/file/list?${new URLSearchParams({ path })}`, { cache: 'no-store' })
  const body = await res.json().catch(() => null) as
    (FileListResponse & { down?: boolean; error?: string; endpoint?: string }) | null
  if (body !== null && body.down === true) throw coreDown(body, '内核未启动')
  if (body === null || !res.ok) throw new Error(body?.error ?? `HTTP ${res.status}`)
  if (!Array.isArray(body.entries)) throw new Error('内核响应结构异常（entries 缺失）')
  return body
}

/** 文本读取窗口（契约 v0.2 /api/file/read）：binary:true 时无 content；>2MB → 413。 */
async function getFileRead(path: string, offset: number, lines: number, signal?: AbortSignal): Promise<FileReadResponse> {
  const params = new URLSearchParams({ path, offset: String(offset), lines: String(lines) })
  const res = await fetch(`/dsh-anyfind/api/file/read?${params}`, { cache: 'no-store', signal })
  const body = await res.json().catch(() => null) as
    (FileReadResponse & { down?: boolean; error?: string; endpoint?: string }) | null
  if (body !== null && body.down === true) throw coreDown(body, '内核未启动')
  if (body === null || !res.ok) throw new Error(body?.error ?? `HTTP ${res.status}`)
  if (typeof body.binary !== 'boolean') throw new Error('内核响应结构异常（binary 缺失）')
  return body
}

/** base64 解字节（与桌面 b64ToBytes 同型）。 */
function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

/**
 * 字节通路（契约 v0.2 补丁 /api/file/read?format=base64）：≤2MB 整文件 base64，
 * 文本与二进制通用；>2MB → 413。响应里 content_base64 在场即用
 * （binary 标志在 base64 通路下可能缺席，不作强校验）。
 */
async function getFileBase64(path: string, signal?: AbortSignal): Promise<FileReadResponse> {
  const params = new URLSearchParams({ path, format: 'base64' })
  const res = await fetch(`/dsh-anyfind/api/file/read?${params}`, { cache: 'no-store', signal })
  const body = await res.json().catch(() => null) as
    (FileReadResponse & { down?: boolean; error?: string; endpoint?: string }) | null
  if (body !== null && body.down === true) throw coreDown(body, '内核未启动')
  if (body === null || !res.ok) throw new Error(body?.error ?? `HTTP ${res.status}`)
  if (typeof body.content_base64 !== 'string' || body.content_base64 === '') {
    throw new Error('内核未返回 content_base64')
  }
  return body
}

/** 图片内联显示上限（>1MB 不拼 data URL，走「打开」降级）。 */
const PREVIEW_IMAGE_MAX_BYTES = 1024 * 1024

const IMAGE_MIME: Readonly<Record<string, string>> = {
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.bmp': 'image/bmp',
  '.ico': 'image/x-icon',
  '.avif': 'image/avif',
}

function imageMime(ext: string): string {
  return IMAGE_MIME[ext] ?? 'application/octet-stream'
}

/** 预览字节（/dsh-anyfind/api/file/bytes，插件自有端点；400/403/404/413 带 error 文案）。 */
async function getFileBytes(path: string, signal?: AbortSignal): Promise<Uint8Array> {
  const res = await fetch(`/dsh-anyfind/api/file/bytes?${new URLSearchParams({ path })}`, { cache: 'no-store', signal })
  if (!res.ok) throw new Error(await errorText(res))
  return new Uint8Array(await res.arrayBuffer())
}

/** Office→PDF（/dsh-anyfind/api/office/pdf，宿主 officeKit/officeToPdf；503=能力不可用）。 */
async function getOfficePdf(path: string, signal?: AbortSignal): Promise<Uint8Array> {
  const res = await fetch(`/dsh-anyfind/api/office/pdf?${new URLSearchParams({ path })}`, { cache: 'no-store', signal })
  if (!res.ok) throw new Error(await errorText(res))
  return new Uint8Array(await res.arrayBuffer())
}

/** 错误响应优先取 JSON error 文案，回退 HTTP 状态。 */
async function errorText(res: Response): Promise<string> {
  const body = await res.json().catch(() => null) as { error?: string } | null
  return typeof body?.error === 'string' ? `${res.status} ${body.error}` : `HTTP ${res.status}`
}

// ---------------------------------------------------------------------------
// DSL token 染色：透明 input 下的高亮背板
// ---------------------------------------------------------------------------

type TintToken = { text: string; cls: string | null }

/** 把查询串切成染色 token：field: 名/值分色、"短语"、-排除、OR 算子、裸词原色。 */
function tintQuery(q: string): TintToken[] {
  const out: TintToken[] = []
  const re = /(\s+)|("[^"]*"?)|(-(?:"[^"]*"|\S+))|\b(OR)\b|((?:kind|agent|project|date|ext|in|role|size):(?:"[^"]*"|\S*))|(\S+)/g
  let m: RegExpExecArray | null
  let last = 0
  while ((m = re.exec(q)) !== null) {
    if (m.index > last) out.push({ text: q.slice(last, m.index), cls: null })
    last = re.lastIndex
    const [tok, ws, phrase, excl, op, field] = m
    if (ws !== undefined) { out.push({ text: ws, cls: null }); continue }
    if (phrase !== undefined) { out.push({ text: phrase, cls: 'rc-tk-phrase' }); continue }
    if (excl !== undefined) { out.push({ text: excl, cls: 'rc-tk-excl' }); continue }
    if (op !== undefined) { out.push({ text: op, cls: 'rc-tk-op' }); continue }
    if (field !== undefined) {
      const ci = tok.indexOf(':')
      out.push({ text: tok.slice(0, ci + 1), cls: 'rc-tk-name' })
      if (ci + 1 < tok.length) out.push({ text: tok.slice(ci + 1), cls: 'rc-tk-val' })
      continue
    }
    out.push({ text: tok, cls: null })
  }
  if (last < q.length) out.push({ text: q.slice(last), cls: null })
  return out
}

/** 语法卡条目（点击插入输入框）。 */
const SYNTAX_ROWS: ReadonlyArray<{ ins: string; desc: string }> = [
  { ins: 'kind:', desc: 'file / content / session / skill / memory' },
  { ins: 'agent:', desc: 'claude-code / codex / dsh / opencode / pi / cursor' },
  { ins: 'project:', desc: '按项目名，如 project:filefind' },
  { ins: 'date:', desc: 'date:2026-10-01..2026-10-20 或 date:7d' },
  { ins: 'ext:', desc: '按扩展名，如 ext:.py、ext:md' },
  { ins: 'in:', desc: '限定目录，如 in:D:/coding' },
  { ins: 'role:', desc: '会话内角色，如 role:user（仅会话组）' },
  { ins: 'size:', desc: '按文件大小，如 size:>1MB（仅文件组）' },
  { ins: '"', desc: '短语精确匹配，如 "hello world"' },
  { ins: 'OR', desc: '或条件，如 nginx OR caddy' },
  { ins: '-', desc: '排除词，如 -test -kind:file' },
]

// ---------------------------------------------------------------------------
// 分段控件（desktop.html 移植）
// ---------------------------------------------------------------------------

const SEG_DEFS: ReadonlyArray<{ k: string; label: string; accent: string; icon: string }> = [
  { k: '', label: '全部', accent: '#9db4e8', icon: '<rect x="2" y="2" width="4.2" height="4.2" rx="1.2"/><rect x="7.8" y="2" width="4.2" height="4.2" rx="1.2"/><rect x="2" y="7.8" width="4.2" height="4.2" rx="1.2"/><rect x="7.8" y="7.8" width="4.2" height="4.2" rx="1.2"/>' },
  { k: 'file', label: '文件', accent: '#6ea8ff', icon: '<path d="M3.8 1.8h4l3.4 3.4v6.2a1.4 1.4 0 0 1-1.4 1.4H3.8a1.4 1.4 0 0 1-1.4-1.4V3.2a1.4 1.4 0 0 1 1.4-1.4z"/><path d="M7.8 1.8v3.4h3.4"/>' },
  { k: 'content', label: '内容', accent: '#2dd4bf', icon: '<path d="M2.6 3.6h8.8"/><path d="M2.6 6.6h8.8"/><path d="M2.6 9.6h5.4"/>' },
  { k: 'session', label: 'AI 会话', accent: '#f472b6', icon: '<rect x="1.9" y="2.4" width="10.2" height="6.8" rx="2.2"/><path d="M5 9.2l-1.1 2.6 3.4-2.1"/>' },
  { k: 'skill', label: '技能', accent: '#fbbf24', icon: '<path d="M7.6 1.4L3.2 7.9h3.3l-.9 4.7 5.2-6.8H7.4z"/>' },
  { k: 'memory', label: '记忆', accent: '#a78bfa', icon: '<path d="M7 2.6v8.8"/><path d="M7 3.6C6.5 2.7 5.6 2.1 4.6 2.1 3.2 2.1 2.1 4.6c0 .5.13.9.35 1.3A2.4 2.4 0 0 0 2 8.1c0 1.3 1.1 2.4 2.4 2.4.9 0 1.7-.4 2.1-1.1"/><path d="M7 3.6c.5-.9 1.4-1.5 2.4-1.5 1.4 0 2.5 1.1 2.5 2.5 0 .5-.13.9-.35 1.3.4.6.46 1.3.46 2.2 0 1.3-1.1 2.4-2.4 2.4-.9 0-1.7-.4-2.1-1.1"/>' },
]

function fmtCount(n: number | undefined | null): string {
  if (n === undefined || n === null || n <= 0) return ''
  if (n >= 1e8) return `${(n / 1e8).toFixed(1).replace(/\.0$/, '')} 亿`
  if (n >= 1e4) return `${(n / 1e4).toFixed(1).replace(/\.0$/, '')} 万`
  return String(n)
}

/** 分段控件：玻璃容器 + 滑动指示器 + 图标 + 计数角标；超宽横向滚动兜底。 */
function SegControl({ active, counts, onChange }: {
  active: string
  counts: Record<string, string>
  onChange: (k: string) => void
}): ReturnType<typeof createElement> {
  const segRef = useRef<HTMLDivElement | null>(null)
  const indRef = useRef<HTMLDivElement | null>(null)
  const btnRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  // 滑动指示器定位（激活段/角标/容器尺寸变化时重测；浮层/隐藏重显时 ResizeObserver 纠正）
  useLayoutEffect(() => {
    const seg = segRef.current
    const ind = indRef.current
    if (seg === null || ind === null) return
    const measure = (): void => {
      const btn = btnRefs.current[active]
      if (btn === null || btn === undefined) return
      ind.style.left = `${btn.offsetLeft}px`
      ind.style.width = `${btn.offsetWidth}px`
      const def = SEG_DEFS.find((d) => d.k === active)
      if (def !== undefined) seg.style.setProperty('--rc-accent', def.accent)
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(seg)
    return () => { ro.disconnect() }
  }, [active, counts])

  return createElement('div', { className: 'rc-seg-scroll' },
    createElement('div', { className: 'rc-seg', ref: segRef, role: 'tablist' },
      createElement('div', { className: 'rc-seg-ind', ref: indRef }),
      ...SEG_DEFS.map((d) => createElement('button', {
        key: d.k || 'all',
        className: `rc-seg-btn${active === d.k ? ' active' : ''}`,
        role: 'tab',
        'aria-selected': active === d.k,
        style: { '--sa': d.accent } as Record<string, string>,
        ref: (el: HTMLButtonElement | null) => { btnRefs.current[d.k] = el },
        onClick: () => { onChange(d.k) },
        onKeyDown: (e: { key: string; preventDefault: () => void }) => {
          if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
          e.preventDefault()
          const i = SEG_DEFS.findIndex((x) => x.k === active)
          const j = (i + (e.key === 'ArrowRight' ? 1 : -1) + SEG_DEFS.length) % SEG_DEFS.length
          onChange(SEG_DEFS[j].k)
          btnRefs.current[SEG_DEFS[j].k]?.focus()
        },
      },
        createElement('svg', {
          className: 'rc-seg-ico', viewBox: '0 0 14 14', fill: 'none', stroke: 'currentColor',
          strokeWidth: 1.3, strokeLinecap: 'round', strokeLinejoin: 'round',
          dangerouslySetInnerHTML: { __html: d.icon },
        }),
        createElement('span', null, d.label),
        createElement('span', { className: 'rc-seg-count' }, counts[d.k] ?? ''),
      )),
    ),
  )
}

// ---------------------------------------------------------------------------
// 展示件
// ---------------------------------------------------------------------------

function AnyfindMark({ size = 16 }: { size?: number }): ReturnType<typeof createElement> {
  return createElement('svg', { width: size, height: size, viewBox: '0 0 24 24', 'aria-hidden': true, fill: 'none', stroke: 'currentColor', strokeWidth: 2.4, strokeLinecap: 'round' },
    createElement('circle', { cx: 10.5, cy: 10.5, r: 6.5 }),
    createElement('line', { x1: 15.5, y1: 15.5, x2: 21, y2: 21 }),
  )
}

/** 命中高亮：escape 后仅恢复 <mark>（内核契约高亮标签），防索引内容注入。 */
function Snippet({ text }: { text: string }): ReturnType<typeof createElement> {
  return createElement('div', { className: 'rc-hit-s', dangerouslySetInnerHTML: { __html: escapeWithMark(text) } })
}

function HitRow({ hit, focused, cmd, busyKey, onResume, onCopy, onDown, onPreview, onReference, onMenu, onCtrlSelect }: {
  hit: Hit
  focused: boolean
  cmd: string | null
  busyKey: boolean
  onResume: (hit: Hit) => void
  onCopy: (text: string, tip: string) => void
  onDown: (endpoint?: string) => void
  onPreview: (hit: Hit) => void
  onReference: (path: string) => void
  onMenu: (e: ContextMenuLike, hit: Hit) => void
  onCtrlSelect: (path: string) => void
}): ReturnType<typeof createElement> {
  const [err, setErr] = useState<string | null>(null)
  const rowRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (focused) rowRef.current?.scrollIntoView({ block: 'nearest' })
  }, [focused])

  const meta = [hit.agent, hit.project, fmtMtime(hit.mtime)].filter((x) => x !== null && x !== '')
  const hasPath = hit.path !== null && hit.path !== ''
  return createElement('div', {
    className: `rc-hit${focused ? ' focused' : ''}`,
    ref: rowRef,
    draggable: hasPath,
    onDragStart: hasPath
      ? (e: DragStartLike) => {
          startFileDrag(e, {
            path: hit.path ?? '',
            name: hit.title ?? fsBasename(hit.path ?? ''),
            type: 'file',
          })
        }
      : undefined,
    onContextMenu: hasPath ? (e: ContextMenuLike) => { onMenu(e, hit) } : undefined,
    onClick: hasPath
      ? (e: { ctrlKey: boolean; metaKey: boolean; preventDefault(): void }) => {
          if (!e.ctrlKey && !e.metaKey) return
          e.preventDefault()
          onCtrlSelect(hit.path ?? '')
        }
      : undefined,
  },
    createElement('div', { className: 'rc-hit-t' }, hit.title ?? hit.path ?? '(无标题)'),
    meta.length > 0 ? createElement('div', { className: 'rc-hit-meta' }, ...meta.map((m, i) => createElement('span', { key: i }, m))) : null,
    hit.snippet !== null && hit.snippet !== '' ? createElement(Snippet, { text: hit.snippet }) : null,
    hit.path !== null && hit.path !== hit.title ? createElement('div', { className: 'rc-hit-path' }, hit.path) : null,
    hasPath
      ? createElement('div', { className: 'rc-hit-ops' },
          createElement('button', { className: 'rc-btn', onClick: () => { onPreview(hit) } }, '预览'),
          createElement('button', { className: 'rc-btn', onClick: () => { onReference(hit.path ?? '') } }, '引用'),
        )
      : null,
    hit.session_id !== null && hit.agent !== null
      ? createElement('button', { className: 'rc-btn rc-resume', disabled: busyKey,
          onClick: () => {
            void postResume(hit).then(
              (command) => { onResume({ ...hit }) ; onCopy(command, '已复制 resume 命令') },
              (e: unknown) => {
                if (e instanceof CoreDown) { onDown(e.endpoint); return }
                setErr(e instanceof Error ? e.message : String(e))
              },
            )
          } }, busyKey ? '生成中…' : '↩ 跳回会话')
      : null,
    err !== null ? createElement('div', { className: 'rc-banner rc-banner-err' }, err) : null,
    cmd !== null
      ? createElement('div', { style: { display: 'flex', gap: 6, alignItems: 'center' } },
          createElement('span', { className: 'rc-cmd', style: { flex: 1 } }, cmd),
          createElement('button', { className: 'rc-btn', style: { fontSize: 11, padding: '3px 10px' },
            onClick: () => { onCopy(cmd, '已复制') } }, '复制'))
      : null,
  )
}

/** 内核未启动引导页。 */
function DownGuide({ detail, onRetry }: { detail: string; onRetry: () => void }): ReturnType<typeof createElement> {
  return createElement('div', { className: 'rc-guide' },
    createElement('div', { className: 'rc-guide-t' }, '搜索服务未连接'),
    createElement('div', { className: 'rc-banner rc-banner-info' },
      '请先启动 AnyFind 桌面版。本地搜索服务会自动启动，无需配置地址或端口。'),
    detail !== '' ? createElement('div', { className: 'rc-banner rc-banner-err' }, detail) : null,
    createElement('button', { className: 'rc-btn rc-btn-pri', style: { alignSelf: 'flex-start' }, onClick: onRetry }, '⟳ 重新检测'),
  )
}

/** 加载态：结果区居中 spinner + skeleton 占位行。 */
function LoadingState(): ReturnType<typeof createElement> {
  return createElement('div', { className: 'rc-loading' },
    createElement('div', { className: 'rc-loading-t' }, createElement('span', { className: 'rc-spin' }), '搜索中…'),
    createElement('div', { className: 'rc-skel' },
      createElement('div', { className: 'rc-skel-row' }),
      createElement('div', { className: 'rc-skel-row' }),
      createElement('div', { className: 'rc-skel-row' }),
    ),
  )
}

// ---------------------------------------------------------------------------
// 面板（浮层主体；上下键导航 + Enter 主动作）
// ---------------------------------------------------------------------------

function AnyfindPanel(): ReturnType<typeof createElement> {
  const [health, setHealth] = useState<HealthState | null>(null)
  const [downDetail, setDownDetail] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [activeKind, setActiveKind] = useState('')
  const [resp, setResp] = useState<SearchResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [synOpen, setSynOpen] = useState(false)
  const [sel, setSel] = useState(-1)
  const [cmds, setCmds] = useState<Record<string, string>>({})
  const [resumeBusy, setResumeBusy] = useState<string | null>(null)
  const toast = useSyncExternalStore(toastStore.subscribe, toastStore.get)
  const seq = useRef(0)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const hlRef = useRef<HTMLDivElement | null>(null)

  const checkHealth = (): void => {
    void getHealth().then(
      (h) => { setHealth(h); setDownDetail(null) },
      (e: unknown) => {
        setHealth(null)
        setDownDetail(e instanceof Error ? e.message : String(e))
      },
    )
  }
  useEffect(checkHealth, [])

  /** 一次搜索缓存全部 groups；kind 切换不触发请求。 */
  const search = (): void => {
    const q = query.trim()
    if (q === '' || busy) return
    const my = ++seq.current
    setBusy(true)
    setError(null)
    setSynOpen(false)
    void getSearch(q).then(
      (r) => { if (seq.current === my) { setResp(r); setBusy(false); setSel(0) } },
      (e: unknown) => {
        if (seq.current !== my) return
        setBusy(false)
        if (e instanceof CoreDown) {
          setHealth(null)
          setDownDetail(e.message)
                    return
        }
        setError(e instanceof Error ? e.message : String(e))
      },
    )
  }

  /** 语法卡点击插入：插到光标处（无光标则追加；与前词之间补空格），随后聚焦。 */
  const insertSyntax = (ins: string): void => {
    const input = inputRef.current
    const at = input !== null && document.activeElement === input ? input.selectionStart : null
    setQuery((prev) => {
      const pos = at ?? prev.length
      const glue = pos > 0 && prev[pos - 1] !== ' ' ? ' ' : ''
      return `${prev.slice(0, pos)}${glue}${ins}${prev.slice(pos)}`
    })
    requestAnimationFrame(() => {
      inputRef.current?.focus()
      if (inputRef.current !== null && at !== null) {
        const p = at + ins.length
        inputRef.current.setSelectionRange(p, p)
      }
    })
  }

  const copyText = (text: string, tip: string): void => {
    void navigator.clipboard.writeText(text).then(
      () => { toastStore.show(tip) },
      () => { toastStore.show('复制失败（剪贴板权限），请手动复制') },
    )
  }

  /** Enter 主动作：会话命中=生成 resume 命令并复制；其余=复制路径。 */
  const primaryAction = (key: string, hit: Hit): void => {
    if (hit.session_id !== null && hit.agent !== null) {
      setResumeBusy(key)
      void postResume(hit).then(
        (command) => {
          setResumeBusy(null)
          setCmds((prev) => ({ ...prev, [key]: command }))
          copyText(command, '已复制 resume 命令')
        },
        (e: unknown) => {
          setResumeBusy(null)
          if (e instanceof CoreDown) {
            setHealth(null)
            setDownDetail('内核连接中断')
                        return
          }
          setError(e instanceof Error ? e.message : String(e))
        },
      )
      return
    }
    if (hit.path !== null) copyText(hit.path, '已复制路径')
  }

  // 分段角标：搜索后=各组命中数（截断带 +）；空闲=health 索引总量
  const counts: Record<string, string> = {}
  if (resp !== null) {
    let sum = 0
    let anyTrunc = false
    for (const d of SEG_DEFS) {
      if (d.k === '') continue
      const n = (resp.groups[d.k] ?? []).length
      const trunc = resp.truncated?.[d.k] === true
      counts[d.k] = n > 0 ? `${n}${trunc ? '+' : ''}` : ''
      sum += n
      if (trunc) anyTrunc = true
    }
    counts[''] = sum > 0 ? `${sum}${anyTrunc ? '+' : ''}` : ''
  } else if (health?.index !== undefined) {
    const idx = health.index
    counts[''] = fmtCount((idx.files ?? 0) + (idx.sessions ?? 0) + (idx.skills ?? 0) + (idx.memories ?? 0))
    counts.file = fmtCount(idx.files)
    counts.session = fmtCount(idx.sessions)
    counts.skill = fmtCount(idx.skills)
    counts.memory = fmtCount(idx.memories)
  }

  // 纯客户端过滤：全部=各非空组；单 kind=该组（空组给空态提示）
  const visibleGroups = resp === null
    ? []
    : (activeKind === '' ? KINDS : [activeKind])
        .map((k) => ({ kind: k, hits: resp.groups[k] ?? [] }))
        .filter((g) => activeKind !== '' || g.hits.length > 0)
  const total = resp === null ? 0 : KINDS.reduce((n, k) => n + (resp.groups[k] ?? []).length, 0)

  // 上下键导航用的扁平命中表（key 与渲染一致）
  const flatHits: Array<{ key: string; hit: Hit }> = visibleGroups.flatMap((g) =>
    g.hits.map((h, i) => ({ key: `${g.kind}-${i}`, hit: h })))
  const selClamped = sel >= 0 && sel < flatHits.length ? sel : -1

  const onInputKeyDown = (e: { key: string; preventDefault: () => void }): void => {
    if (e.key === 'Enter') {
      e.preventDefault()
      if (selClamped >= 0) primaryAction(flatHits[selClamped].key, flatHits[selClamped].hit)
      else search()
      return
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (flatHits.length === 0) return
      e.preventDefault()
      const delta = e.key === 'ArrowDown' ? 1 : -1
      setSel((prev) => {
        const cur = prev >= 0 && prev < flatHits.length ? prev : (delta === 1 ? -1 : 0)
        return Math.min(Math.max(cur + delta, 0), flatHits.length - 1)
      })
    }
  }

  const tint = tintQuery(query)

  return createElement('div', { className: 'rc-panel' },
    createElement(StyleTag),
    createElement('div', { className: 'rc-head' },
      createElement('div', { className: 'rc-input-wrap' },
        createElement('div', { className: 'rc-input-hl', ref: hlRef, 'aria-hidden': true },
          ...tint.map((t, i) => t.cls === null
            ? createElement('span', { key: i }, t.text)
            : createElement('span', { key: i, className: t.cls }, t.text)),
          // 尾部零宽占位保证末尾空格可见宽度同步
          createElement('span', null, '​'),
        ),
        createElement('input', {
          className: 'rc-input',
          ref: inputRef,
          placeholder: '搜全部历史：文件 / 会话 / 技能 / 记忆…',
          value: query,
          spellCheck: false,
          onChange: (e: { target: { value: string } }) => { setQuery(e.target.value) },
          onScroll: () => {
            if (inputRef.current !== null && hlRef.current !== null) {
              hlRef.current.scrollLeft = inputRef.current.scrollLeft
            }
          },
          onKeyDown: onInputKeyDown,
        }),
        createElement('button', {
          className: 'rc-syn-btn',
          title: '过滤语法',
          onClick: () => { setSynOpen((v) => !v) },
        }, '?'),
      ),
      createElement('button', { className: 'rc-btn rc-btn-pri', onClick: search, disabled: busy || query.trim() === '' }, '搜索'),
      synOpen
        ? createElement('div', { className: 'rc-syn' },
            createElement('div', { className: 'rc-syn-t' }, '过滤语法（点击插入）'),
            ...SYNTAX_ROWS.map((r) => createElement('div', {
              key: r.ins,
              className: 'rc-syn-row',
              onClick: () => { insertSyntax(r.ins) },
            },
              createElement('code', null, r.ins),
              createElement('span', null, r.desc),
            )),
            createElement('div', { className: 'rc-syn-ex' }, '组合示例：', createElement('code', null, 'kind:session agent:codex "nginx 反代" -test')),
          )
        : null,
    ),
    createElement(SegControl, { active: activeKind, counts, onChange: (k) => { setActiveKind(k); setSel(0) } }),
    createElement('div', { className: 'rc-body' },
      health === null
        ? createElement(DownGuide, { detail: downDetail ?? '', onRetry: checkHealth })
        : null,
      busy ? createElement(LoadingState) : null,
      !busy && health !== null && resp === null
        ? createElement('div', { className: 'rc-hit-meta' },
            createElement('span', { className: 'rc-meta-pill' }, `● 内核就绪${health.version !== undefined ? ` v${health.version}` : ''}`),
            health.index !== undefined
              ? createElement('span', null, `索引：文件 ${health.index.files ?? 0} · 会话 ${health.index.sessions ?? 0} · 技能 ${health.index.skills ?? 0} · 记忆 ${health.index.memories ?? 0}`)
              : null,
            createElement('span', { style: { flex: 1 } }),
            createElement('span', { className: 'rc-kbd' }, '↑↓ 导航 · Enter 执行 · Esc 关闭'),
          )
        : null,
      error !== null ? createElement('div', { className: 'rc-banner rc-banner-err' }, error) : null,
      !busy && resp !== null && total === 0
        ? createElement('div', { className: 'rc-banner rc-banner-info' }, `「${resp.q}」没有命中——换个关键词，或去掉过滤条件。`)
        : null,
      !busy && resp !== null && total > 0 && activeKind !== '' && visibleGroups.every((g) => g.hits.length === 0)
        ? createElement('div', { className: 'rc-banner rc-banner-info' }, `「${resp.q}」在「${KIND_LABEL[activeKind]}」组没有命中（全部 ${total} 条在其他组）。`)
        : null,
      !busy && resp !== null && total > 0 && activeKind === ''
        ? createElement('div', { className: 'rc-hit-meta' }, `命中 ${total} 条${resp.took_ms !== undefined ? ` · ${resp.took_ms}ms` : ''}`)
        : null,
      ...(!busy ? visibleGroups.map((g) => createElement('div', { key: g.kind, className: 'rc-group' },
        createElement('div', { className: 'rc-group-t' }, `${KIND_LABEL[g.kind]}（${g.hits.length}）`),
        ...g.hits.map((h, i) => {
          const key = `${g.kind}-${i}`
          const flatIdx = flatHits.findIndex((f) => f.key === key)
          return createElement(HitRow, {
            key,
            hit: h,
            focused: flatIdx === selClamped,
            cmd: cmds[key] ?? null,
            busyKey: resumeBusy === key,
            onResume: (hh) => { primaryAction(key, hh) },
            onCopy: copyText,
            onDown: () => {
              setHealth(null)
              setDownDetail('内核连接中断')
            },
            onPreview: (hh) => {
              if (hh.path === null || hh.path === '') return
              filesOpenPreview(hh.path, hh.title ?? fsBasename(hh.path), false)
              overlayStore.setView('files')
            },
            onReference: (path) => { if (path !== '') referenceOrCopy(path) },
            onMenu: (e, hh) => {
              if (hh.path === null || hh.path === '') return
              // 契约 Hit 无 is_dir 字段 → 结果行不给「新建」入口（桌面端靠 hit.is_dir 判定）。
              openContextMenu(e, hh.path, fsParent(hh.path), false)
            },
            onCtrlSelect: (path) => { if (path !== '') filesToggleMulti(path) },
          })
        }),
      )) : []),
    ),
    toast !== '' ? createElement('div', { className: 'rc-toast' }, toast) : null,
    createElement(FileContextMenu),
  )
}

// ---------------------------------------------------------------------------
// 文件树 + 多标签：模块级 store（浮层「文件」视图与 betterSidebar 文件 tab 共用）
// 标签操作（新建/关闭/切换/排序）纯客户端状态零请求；list 只在目录首次展开/进入
// 时懒加载一次，之后命中缓存。
// ---------------------------------------------------------------------------

/** 契约同款噪声目录：仅隐藏目录本身、大小写不敏感（内核 noise=0 已滤，客户端再滤一层）。 */
const NOISE_DIRS: ReadonlySet<string> = new Set([
  '.git', 'node_modules', 'dist', 'build', 'coverage', '.idea', 'target',
])

/** 新标签默认根目录（本工作区）；路径栏可跳任意绝对路径。 */
const DEFAULT_ROOT = 'D:/coding'

interface FileTab {
  id: number
  root: string
  history: string[]
  selected: string | null
  /** Ctrl 点选批量集合（含单选；右键落在集合内 → 整批操作）。 */
  multi: string[]
}

interface DirListState {
  status: 'loading' | 'ready' | 'error'
  entries: FileEntry[]
  truncated: boolean
  error: string
}

/** 预览目标（模块级，浮层与侧栏 tab 共享；offset/lines = file/read 分页窗口）。 */
interface PreviewState {
  path: string
  name: string
  isDir: boolean
  offset: number
  lines: number
}

interface FilesState {
  tabs: FileTab[]
  activeId: number
  expanded: string[]
  lists: Record<string, DirListState>
  preview: PreviewState | null
  nextId: number
}

const files = {
  state: { tabs: [], activeId: -1, expanded: [], lists: {}, preview: null, nextId: 1 } as FilesState,
  listeners: new Set<() => void>(),
  seq: {} as Record<string, number>,
}

function emitFiles(): void {
  for (const l of files.listeners) l()
}

function setFilesState(fn: (s: FilesState) => FilesState): void {
  const next = fn(files.state)
  if (next === files.state) return
  files.state = next
  emitFiles()
}

function filesSubscribe(fn: () => void): () => void {
  files.listeners.add(fn)
  return () => { files.listeners.delete(fn) }
}

function filesGetSnapshot(): FilesState {
  return files.state
}

function findTab(s: FilesState, id: number): FileTab | null {
  return s.tabs.find((t) => t.id === id) ?? null
}

function currentDir(t: FileTab): string {
  return t.history[t.history.length - 1] ?? t.root
}

/** 新建标签（纯状态）：开在当前标签的目录上（无标签时开工作区根），随后由面板懒加载。 */
function filesNewTab(root?: string): void {
  const base = (root ?? DEFAULT_ROOT).trim()
  if (base === '') return
  setFilesState((s) => {
    const id = s.nextId
    const tab: FileTab = { id, root: base, history: [base], selected: null, multi: [] }
    return { ...s, tabs: [...s.tabs, tab], activeId: id, nextId: id + 1 }
  })
}

/** 关闭标签（纯状态）：关的是激活项则激活右侧邻居（无则左侧，全关则 -1）。 */
function filesCloseTab(id: number): void {
  setFilesState((s) => {
    const i = s.tabs.findIndex((t) => t.id === id)
    if (i < 0) return s
    const tabs = s.tabs.filter((t) => t.id !== id)
    const fallback = tabs.length === 0 ? -1 : tabs[Math.min(i, tabs.length - 1)].id
    return { ...s, tabs, activeId: s.activeId === id ? fallback : s.activeId }
  })
}

function filesActivate(id: number): void {
  setFilesState((s) => (s.activeId === id || findTab(s, id) === null ? s : { ...s, activeId: id }))
}

/** 标签排序（纯状态）：delta = -1 左移 / +1 右移，越界 no-op。 */
function filesMoveTab(id: number, delta: number): void {
  setFilesState((s) => {
    const i = s.tabs.findIndex((t) => t.id === id)
    const j = i + delta
    if (i < 0 || j < 0 || j >= s.tabs.length) return s
    const tabs = [...s.tabs]
    const moved = tabs[i]
    tabs.splice(i, 1)
    tabs.splice(j, 0, moved)
    return { ...s, tabs }
  })
}

/** 进入目录：压入当前标签的 history（同目录 no-op），随后懒加载。 */
function filesNavigate(path: string): void {
  const target = path.trim()
  if (target === '') return
  setFilesState((s) => {
    const t = findTab(s, s.activeId)
    if (t === null || currentDir(t) === target) return s
    return { ...s, tabs: s.tabs.map((x) => (x.id === t.id ? { ...x, history: [...x.history, target] } : x)) }
  })
  filesEnsureLoaded(target)
}

/** 历史后退：弹出 history 尾项（root 常驻栈底，弹不掉）。 */
function filesBack(): void {
  setFilesState((s) => {
    const t = findTab(s, s.activeId)
    if (t === null || t.history.length <= 1) return s
    const history = t.history.slice(0, -1)
    return { ...s, tabs: s.tabs.map((x) => (x.id === t.id ? { ...x, history } : x)) }
  })
}

/** 展开/收起目录（纯状态）；展开时懒加载子目录 list。 */
function filesToggleExpand(path: string): void {
  const willExpand = !files.state.expanded.includes(path)
  setFilesState((s) => ({
    ...s,
    expanded: willExpand ? [...s.expanded, path] : s.expanded.filter((p) => p !== path),
  }))
  if (willExpand) filesEnsureLoaded(path)
}

/** 选中文件（纯状态，零请求）。 */
function filesSelect(path: string | null): void {
  setFilesState((s) => {
    const t = findTab(s, s.activeId)
    if (t === null) return s
    const multi = path === null ? [] : [path]
    if (t.selected === path && t.multi.length === multi.length && t.multi[0] === multi[0]) return s
    return { ...s, tabs: s.tabs.map((x) => (x.id === t.id ? { ...x, selected: path, multi } : x)) }
  })
}

/** Ctrl/Meta 点选批量：在集合里则移出，否则加入（上限 128，超限提示不入集）。 */
function filesToggleMulti(path: string): void {
  setFilesState((s) => {
    const t = findTab(s, s.activeId)
    if (t === null) return s
    const has = t.multi.includes(path)
    if (!has && t.multi.length >= FILE_OP_MAX_PATHS) {
      toastStore.show(`批量选择超过 ${FILE_OP_MAX_PATHS} 项上限，先取消部分再选`)
      return s
    }
    const multi = has ? t.multi.filter((p) => p !== path) : [...t.multi, path]
    const selected = has ? (t.selected === path ? multi[multi.length - 1] ?? null : t.selected) : path
    return { ...s, tabs: s.tabs.map((x) => (x.id === t.id ? { ...x, multi, selected } : x)) }
  })
}

/**
 * 右键前的选区归位：落在已有批量集合内 → 保留整批；否则收成单选该项。
 * 返回该次右键应操作的路径集合。
 */
function filesContextSelect(path: string): string[] {
  const st = files.state
  const tab = findTab(st, st.activeId)
  const multi = tab?.multi ?? []
  if (multi.length > 0 && multi.includes(path)) return multi
  filesSelect(path)
  return [path]
}

/** 打开预览（树节点 / 搜索结果行共用）：新开窗口从 offset 0、每页 30 行起。 */
function filesOpenPreview(path: string, name: string, isDir = false): void {
  setFilesState((s) => ({
    ...s,
    preview: { path, name, isDir, offset: 0, lines: 30 },
  }))
}

function filesClosePreview(): void {
  setFilesState((s) => (s.preview === null ? s : { ...s, preview: null }))
}

/** 翻页：deltaLines 为 +lines/-lines（到头钳 0）。 */
function filesPreviewOffset(deltaLines: number): void {
  setFilesState((s) => {
    if (s.preview === null) return s
    const offset = Math.max(0, s.preview.offset + deltaLines)
    if (offset === s.preview.offset) return s
    return { ...s, preview: { ...s.preview, offset } }
  })
}

/** 换每页行数（30/60/120）：回到第一页。 */
function filesPreviewLines(lines: number): void {
  setFilesState((s) => {
    if (s.preview === null) return s
    if (s.preview.lines === lines && s.preview.offset === 0) return s
    return { ...s, preview: { ...s.preview, lines, offset: 0 } }
  })
}

/** 懒加载目录 list：已有记录（loading/ready/error）则跳过，force 用于手动刷新。 */
function filesEnsureLoaded(path: string, force = false): void {
  if (!force && files.state.lists[path] !== undefined) return
  const seq = (files.seq[path] ?? 0) + 1
  files.seq[path] = seq
  const pending: DirListState = { status: 'loading', entries: [], truncated: false, error: '' }
  setFilesState((s) => ({ ...s, lists: { ...s.lists, [path]: pending } }))
  void getFileList(path).then(
    (r) => {
      if (files.seq[path] !== seq) return
      const ready: DirListState = {
        status: 'ready',
        entries: prepareEntries(r.entries),
        truncated: r.truncated === true,
        error: '',
      }
      setFilesState((s) => ({ ...s, lists: { ...s.lists, [path]: ready } }))
    },
    (e: unknown) => {
      if (files.seq[path] !== seq) return
      const failed: DirListState = {
        status: 'error',
        entries: [],
        truncated: false,
        error: e instanceof Error ? e.message : String(e),
      }
      setFilesState((s) => ({ ...s, lists: { ...s.lists, [path]: failed } }))
    },
  )
}

/** 契约过滤 + 排序：噪声目录隐藏（仅目录、大小写不敏感），目录优先按名排。 */
function prepareEntries(entries: readonly FileEntry[]): FileEntry[] {
  return entries
    .filter((e) => !(e.is_dir && NOISE_DIRS.has(e.name.toLowerCase())))
    .sort((a, b) => {
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1
      const al = a.name.toLowerCase()
      const bl = b.name.toLowerCase()
      if (al !== bl) return al < bl ? -1 : 1
      return a.name < b.name ? -1 : 1
    })
}

function joinFsPath(dir: string, name: string): string {
  if (dir === '') return name
  const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/'
  const last = dir[dir.length - 1]
  return last === '/' || last === '\\' ? dir + name : dir + sep + name
}

function fsBasename(p: string): string {
  const parts = p.replace(/[\\/]+$/, '').split(/[\\/]/)
  return parts[parts.length - 1] ?? p
}

function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return ''
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`
  return `${(n / 1024 ** 3).toFixed(1)} GB`
}

/** mtime 契约为 "YYYY-MM-DDTHH:MM:SS"（本地时区）→ 展示成 "YYYY-MM-DD HH:MM"。 */
function fmtEntryTime(mtime: string | null): string {
  if (mtime === null || mtime.length < 16) return ''
  return `${mtime.slice(0, 10)} ${mtime.slice(11, 16)}`
}

interface FileRow {
  key: string
  depth: number
  name: string
  path: string
  isDir: boolean
  size: number
  mtime: string | null
  kind: 'entry' | 'loading' | 'error' | 'empty'
  expanded: boolean
}

/** 由展开集 + 已加载 list 生成扁平行（未加载的展开子目录给 loading 行）。 */
function buildFileRows(state: FilesState, root: string): FileRow[] {
  const rows: FileRow[] = []
  const expanded = new Set(state.expanded)
  const walk = (dir: string, depth: number): void => {
    const list = state.lists[dir]
    if (list === undefined || list.status === 'loading') {
      rows.push({ key: `${dir}#loading`, depth, name: '加载中…', path: dir, isDir: false, size: 0, mtime: null, kind: 'loading', expanded: false })
      return
    }
    if (list.status === 'error') {
      rows.push({ key: `${dir}#error`, depth, name: `加载失败：${list.error}`, path: dir, isDir: false, size: 0, mtime: null, kind: 'error', expanded: false })
      return
    }
    if (list.entries.length === 0) {
      rows.push({ key: `${dir}#empty`, depth, name: '（空目录）', path: dir, isDir: false, size: 0, mtime: null, kind: 'empty', expanded: false })
      return
    }
    for (const e of list.entries) {
      const p = joinFsPath(dir, e.name)
      const isOpen = e.is_dir && expanded.has(p)
      rows.push({ key: p, depth, name: e.name, path: p, isDir: e.is_dir, size: e.size, mtime: e.mtime, kind: 'entry', expanded: isOpen })
      if (isOpen) walk(p, depth + 1)
    }
  }
  walk(root, 0)
  return rows
}

function FilesIcon({ size = 22 }: { size?: number }): ReturnType<typeof createElement> {
  return createElement('svg', { width: size, height: size, viewBox: '0 0 24 24', 'aria-hidden': true, fill: 'none', stroke: 'currentColor', strokeWidth: 1.8, strokeLinecap: 'round', strokeLinejoin: 'round' },
    createElement('path', { d: 'M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2.5h7A1.5 1.5 0 0 1 19 10v7.5A1.5 1.5 0 0 1 17.5 19h-13A1.5 1.5 0 0 1 3 17.5z' }),
  )
}

// ---------------------------------------------------------------------------
// 预览（树节点 / 搜索结果行共用）：
//   - 富格式（vendor previewDefinitions 命中）→ /api/file/bytes 取字节 → vendor 正文组件；
//   - 图片 / PDF → /api/file/read?format=base64（契约 v0.2 补丁，≤2MB 整文件）：
//     图片 ≤1MB 拼 data URL 内联（超限/取不到字节 →「打开」降级），
//     PDF atob 解字节喂 pdfjs-dist（>2MB 内核 413 →「用系统应用打开」降级）；
//   - Office(doc/docx/xls/xlsx/ppt/pptx) → /api/office/pdf（宿主 officeKit 转换 → pdfjs），
//     能力/转换失败 → 降级「用系统应用打开（复制路径）」；
//   - 其余 → file/read 文本分页 30/60/120；binary:true 占位。
// ---------------------------------------------------------------------------

const RASTER_IMAGE_EXTS: ReadonlySet<string> = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.ico', '.avif',
])

/** officeKit/officeToPdf 接受的扩展名（与宿主 OFFICE_EXTS 一致）。 */
const OFFICE_PREVIEW_EXTS: ReadonlySet<string> = new Set([
  '.doc', '.docx', '.xls', '.xlsx', '.ppt', '.pptx',
])

/** vendor 预览（src/vendor/VENDORED.md）：扩展名 → previewBodies id。 */
const VENDOR_PREVIEW_BY_EXT: ReadonlyMap<string, string> = (() => {
  const map = new Map<string, string>()
  const defs = previewDefinitions as unknown as Array<{ id: string; extensions: readonly string[] }>
  for (const def of defs) {
    for (const e of def.extensions) map.set(`.${e}`, def.id)
  }
  return map
})()

function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i <= 0 ? '' : name.slice(i).toLowerCase()
}

type PreviewView =
  | { kind: 'loading' }
  | { kind: 'error'; error: string }
  | { kind: 'binary'; reason: 'binary' | 'image-oversize' | 'image-nob64' | 'pdf-nob64' }
  | { kind: 'text'; content: string; totalLines: number; actualLines: number }
  | { kind: 'pdf'; pages: number }
  | { kind: 'image'; src: string; bytes: number }
  | { kind: 'vendor'; id: string; data: Uint8Array }

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** 无二进制端点时的「打开」降级：复制路径 + 提示（记 gap：浏览器拿不到本机文件字节）。 */
function openExternally(path: string): void {
  void navigator.clipboard.writeText(path).then(
    () => { toastStore.show('浏览器无法直接打开本机文件，已复制路径') },
    () => { toastStore.show('复制失败（剪贴板权限），请手动复制') },
  )
}

/** vendor 正文组件 props（src/vendor/ui/views.js 契约：content.data = 原始字节）。 */
interface VendorBodyProps {
  content: { data: Uint8Array }
  resourceAddress?: string
  t?: unknown
}

function FilePreview(): ReturnType<typeof createElement> | null {
  const st = useSyncExternalStore(filesSubscribe, filesGetSnapshot)
  const preview = st.preview
  const [view, setView] = useState<PreviewView>({ kind: 'loading' })
  const [retry, setRetry] = useState(0)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const docRef = useRef<pdfjs.PDFDocumentProxy | null>(null)
  const taskRef = useRef<pdfjs.PDFDocumentLoadingTask | null>(null)

  const path = preview?.path
  const offset = preview?.offset ?? 0
  const lines = preview?.lines ?? 30
  const isDir = preview?.isDir === true

  // 目录预览：直接用已缓存/懒加载的 list（file/read 对目录回 404）。
  useEffect(() => {
    if (path !== undefined && isDir) filesEnsureLoaded(path)
  }, [path, isDir])

  useEffect(() => {
    if (path === undefined || isDir) return
    let cancelled = false
    setView({ kind: 'loading' })
    const name = preview?.name ?? path
    const ext = extOf(name)
    const vendorId = VENDOR_PREVIEW_BY_EXT.get(ext)

    const destroyPdf = (): void => {
      // 上一窗口的 pdf 文档随重取释放（PDFDocumentProxy 无 destroy，按 loadingTask 释放）。
      docRef.current = null
      const task = taskRef.current
      taskRef.current = null
      if (task !== null && !task.destroyed) void task.destroy()
    }

    const loadPdf = (data: Uint8Array): void => {
      const task = pdfjs.getDocument({ data })
      taskRef.current = task
      void task.promise.then(
        (doc) => {
          if (cancelled) { void task.destroy(); return }
          docRef.current = doc
          setView({ kind: 'pdf', pages: doc.numPages })
        },
        (e: unknown) => { if (!cancelled) setView({ kind: 'error', error: errMsg(e) }) },
      )
    }

    const fail = (e: unknown): void => { if (!cancelled) setView({ kind: 'error', error: errMsg(e) }) }

    // 富格式：vendor previewDefinitions 命中 → 字节 → vendor 正文组件。
    if (vendorId !== undefined) {
      void getFileBytes(path).then((data) => { if (!cancelled) setView({ kind: 'vendor', id: vendorId, data }) }, fail)
      return () => { cancelled = true }
    }

    // Office → PDF（宿主 officeKit 转换）。
    if (OFFICE_PREVIEW_EXTS.has(ext)) {
      void getOfficePdf(path).then(loadPdf, fail)
      return () => { cancelled = true; destroyPdf() }
    }

    // 图片 / PDF：内核 format=base64 字节通路（≤2MB 整文件，文本与二进制通用）。
    // 图片 ≤1MB 拼 data URL 内联，超限/取不到字节 → 「打开」降级；PDF atob 解字节喂 pdfjs。
    if (ext === '.pdf' || RASTER_IMAGE_EXTS.has(ext)) {
      void getFileBase64(path).then(
        (r) => {
          if (cancelled) return
          const b64 = r.content_base64 as string
          const bytes = base64ToBytes(b64)
          if (RASTER_IMAGE_EXTS.has(ext)) {
            if (bytes.length > PREVIEW_IMAGE_MAX_BYTES) {
              setView({ kind: 'binary', reason: 'image-oversize' })
              return
            }
            setView({ kind: 'image', src: `data:${imageMime(ext)};base64,${b64}`, bytes: bytes.length })
            return
          }
          loadPdf(bytes)
        },
        fail,
      )
      return () => { cancelled = true; destroyPdf() }
    }

    // 其余：内核文本窗口分页。
    void getFileRead(path, offset, lines).then(
      (r) => {
        if (cancelled) return
        if (r.binary === true) {
          setView({ kind: 'binary', reason: 'binary' })
          return
        }
        setView({ kind: 'text', content: r.content ?? '', totalLines: r.totalLines ?? 0, actualLines: r.lines ?? 0 })
      },
      fail,
    )
    return () => { cancelled = true; destroyPdf() }
  }, [path, offset, lines, isDir, retry, preview?.name])

  // PDF 画布挂载后再渲染（canvas 由 view.kind==='pdf' 分支产出）。
  useEffect(() => {
    if (view.kind !== 'pdf') return
    const canvas = canvasRef.current
    const doc = docRef.current
    if (canvas === null || doc === null) return
    let cancelled = false
    void (async () => {
      const page = await doc.getPage(1)
      const viewport = page.getViewport({ scale: 1 })
      const ctx = canvas.getContext('2d')
      if (ctx === null || cancelled) return
      canvas.width = Math.max(1, Math.floor(viewport.width))
      canvas.height = Math.max(1, Math.floor(viewport.height))
      await page.render({ canvas, viewport }).promise
    })().catch((e: unknown) => { if (!cancelled) setView({ kind: 'error', error: errMsg(e) }) })
    return () => { cancelled = true }
  }, [view.kind])

  if (preview === null) return null

  const name = preview.name
  const isRasterImage = RASTER_IMAGE_EXTS.has(extOf(name))

  let body: ReturnType<typeof createElement>
  if (isDir) {
    const list = path !== undefined ? st.lists[path] : undefined
    body = list === undefined || list.status === 'loading'
      ? createElement('div', { className: 'rc-ph' }, '加载目录…')
      : list.status === 'error'
        ? createElement('div', { className: 'rc-banner rc-banner-err' }, list.error)
        : createElement('div', { className: 'rc-pvbody' },
            createElement('div', { className: 'rc-pvinfo', style: { marginBottom: 6 } },
              `目录内容（${list.entries.length} 项，噪声目录已隐藏）`),
            createElement('div', { className: 'rc-pvrows' },
              ...prepareEntries(list.entries).map((e) => createElement('div', { key: e.name, className: 'rc-pvrow' },
                createElement('span', { className: 'n' }, `${e.is_dir ? '▸ ' : ''}${e.name}`),
                createElement('span', { className: 'm' }, e.is_dir ? '—' : fmtBytes(e.size)),
                createElement('span', { className: 'm' }, fmtEntryTime(e.mtime)),
              )),
            ),
          )
  } else if (view.kind === 'loading') {
    body = createElement('div', { className: 'rc-ph' }, createElement('span', { className: 'rc-spin' }), '读取中…')
  } else if (view.kind === 'error') {
    const openable = OFFICE_PREVIEW_EXTS.has(extOf(name)) || isRasterImage || extOf(name) === '.pdf'
    body = createElement('div', { className: 'rc-ph' },
      createElement('div', { className: 'rc-banner rc-banner-err', style: { maxWidth: '100%' } }, view.error),
      createElement('div', { style: { display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' } },
        createElement('button', { className: 'rc-btn rc-btn-pri', onClick: () => { setRetry((n) => n + 1) } }, '⟳ 重试'),
        // office 转换不可用（503/能力缺席/失败）或 图片/PDF 取字节失败（>2MB 413 等）→「打开」降级。
        openable && path !== null && path !== undefined
          ? createElement('button', { className: 'rc-btn', onClick: () => { openExternally(path) } }, '用系统应用打开（复制路径）')
          : null,
      ),
    )
  } else if (view.kind === 'binary') {
    // 降级面：文本通路遇 NUL 二进制，或 base64 通路超限/缺字节 →「打开」。
    const notes: Record<typeof view.reason, string> = {
      binary: '二进制文件（前 8KB 含 NUL），不支持文本预览',
      'image-oversize': `图片超过 ${Math.round(PREVIEW_IMAGE_MAX_BYTES / 1024)}KB（base64 内联上限），不在面板内显示`,
      'image-nob64': '内核未返回图片字节（content_base64 缺失）',
      'pdf-nob64': '内核未返回 PDF 字节（content_base64 缺失），无法渲染',
    }
    body = createElement('div', { className: 'rc-ph' },
      createElement('div', null, notes[view.reason]),
      path !== null && path !== undefined
        ? createElement('button', { className: 'rc-btn rc-btn-pri', onClick: () => { openExternally(path) } }, '打开（复制路径）')
        : null,
    )
  } else if (view.kind === 'pdf') {
    body = createElement('div', { className: 'rc-pvbody' },
      createElement('canvas', { className: 'rc-pvcanvas', ref: canvasRef }),
      createElement('div', { className: 'rc-pvinfo', style: { marginTop: 6, textAlign: 'center' } },
        `PDF 第 1 页 / 共 ${view.pages} 页`),
    )
  } else if (view.kind === 'image') {
    body = createElement('div', { className: 'rc-pvbody rc-pvimg' },
      createElement('img', { src: view.src, alt: name, draggable: false }),
      createElement('div', { className: 'rc-pvinfo', style: { marginTop: 6, textAlign: 'center' } },
        `data URL（${imageMime(extOf(name))}，${fmtBytes(view.bytes)}）`),
    )
  } else if (view.kind === 'vendor') {
    const bodies = previewBodies as unknown as Record<string, unknown>
    const VendorBody = typeof bodies[view.id] === 'function'
      ? bodies[view.id] as (props: VendorBodyProps) => ReturnType<typeof createElement>
      : null
    body = createElement('div', { className: 'rc-pvbody rc-pvrich' },
      VendorBody !== null
        ? createElement(VendorBody, { content: { data: view.data }, resourceAddress: preview.path, t: null })
        : createElement('div', { className: 'rc-ph' }, `预览组件缺失：${view.id}`),
    )
  } else {
    const start = offset + 1
    const end = offset + view.actualLines
    const atEnd = view.totalLines === 0 || end >= view.totalLines
    body = createElement('div', { className: 'rc-prev' },
      createElement('div', { className: 'rc-prev-ctl' },
        createElement('div', { className: 'rc-psz' }, ...[30, 60, 120].map((n) => createElement('button', {
          key: n,
          className: preview.lines === n ? 'on' : '',
          onClick: () => { filesPreviewLines(n) },
        }, String(n)))),
        createElement('button', { className: 'rc-btn', disabled: offset <= 0,
          onClick: () => { filesPreviewOffset(-preview.lines) } }, '← 上一页'),
        createElement('button', { className: 'rc-btn', disabled: atEnd,
          onClick: () => { filesPreviewOffset(preview.lines) } }, '下一页 →'),
        createElement('span', { className: 'rc-pvinfo' },
          view.totalLines === 0 ? '空文件' : `行 ${start}–${end} / ${view.totalLines}`),
      ),
      createElement('div', { className: 'rc-pvbody' },
        createElement('pre', { className: 'rc-pvpre' }, view.content)),
    )
  }

  return createElement('div', { className: 'rc-prev' },
    previewHead(preview, name),
    body,
  )
}

function previewHead(preview: PreviewState, name: string): ReturnType<typeof createElement> {
  return createElement('div', { className: 'rc-prev-head' },
    createElement('span', { className: 'rc-prev-t', title: preview.path },
      `${preview.isDir ? '📁 ' : '📄 '}${name}`),
    createElement('span', { className: 'rc-pvinfo' }, preview.isDir ? '目录' : '预览'),
    createElement('button', { className: 'rc-btn', onClick: filesClosePreview }, '× 关闭'),
  )
}

// ---------------------------------------------------------------------------
// 引用与拖拽：「引用」按钮 → inputActions.setDraft 追加（无桥降级复制+toast）；
// 条目 draggable → application/x-anyfind-file（JSON {path,name,type}），
// document 级 drop：落 composer 卡片追加，否则复制+toast；目录落框先生成
// ├─└─│ 文本树（≤200 行，core.buildTreeText）。
// ---------------------------------------------------------------------------

/** 「引用」统一入口：有输入框桥 → 追加；否则复制路径 + toast（任务降级要求）。 */
function referenceOrCopy(path: string): void {
  const text = formatReference([path])
  if (text === '') return
  if (insertReferenceText(text)) {
    toastStore.show('已追加到输入框')
    return
  }
  void navigator.clipboard.writeText(text).then(
    () => { toastStore.show('已复制路径（输入框未就绪）') },
    () => { toastStore.show('复制失败（剪贴板权限），请手动复制') },
  )
}

const FILE_DRAG_MIME = 'application/x-anyfind-file'
const TREE_WALK_MAX_DEPTH = 4
const TREE_WALK_MAX_NODES = 300

interface FileDragPayload {
  path: string
  name: string
  type: 'file' | 'folder'
}

interface DragStartLike {
  dataTransfer: DataTransfer | null
}

function startFileDrag(e: DragStartLike, payload: FileDragPayload): void {
  const dt = e.dataTransfer
  if (dt === null) return
  dt.setData(FILE_DRAG_MIME, JSON.stringify(payload))
  dt.setData('text/plain', payload.path)
  dt.effectAllowed = 'copy'
}

function readDragPayload(dt: DataTransfer | null): FileDragPayload | null {
  if (dt === null) return null
  const raw = dt.getData(FILE_DRAG_MIME)
  if (raw === '') return null
  try {
    const parsed = JSON.parse(raw) as Partial<FileDragPayload>
    if (typeof parsed.path !== 'string' || parsed.path === '') return null
    return {
      path: parsed.path,
      name: typeof parsed.name === 'string' && parsed.name !== '' ? parsed.name : fsBasename(parsed.path),
      type: parsed.type === 'folder' ? 'folder' : 'file',
    }
  } catch {
    return null
  }
}

/** 目录 → 树节点：懒加载递归，深度 4 / 节点 300 双上限（读不到就当叶子）。 */
async function walkDirToTree(path: string, depth: number, budget: { left: number }): Promise<PreviewTreeNode> {
  const node: PreviewTreeNode = { name: fsBasename(path) }
  budget.left -= 1
  if (depth >= TREE_WALK_MAX_DEPTH || budget.left <= 0) return node
  let entries: FileEntry[] = []
  try {
    entries = prepareEntries((await getFileList(path)).entries)
  } catch {
    return node
  }
  const children: PreviewTreeNode[] = []
  for (const e of entries) {
    if (budget.left <= 0) break
    const childPath = joinFsPath(path, e.name)
    if (e.is_dir) {
      children.push(await walkDirToTree(childPath, depth + 1, budget))
    } else {
      budget.left -= 1
      children.push({ name: e.name })
    }
  }
  if (children.length > 0) node.children = children
  return node
}

async function buildDirTreeText(path: string): Promise<string> {
  const tree = await walkDirToTree(path, 0, { left: TREE_WALK_MAX_NODES })
  return buildTreeText([tree])
}

/** 落点交付：composer 卡片 → setDraft 追加（目录=文本树）；否则复制 + toast。 */
async function deliverDroppedFile(payload: FileDragPayload, onComposer: boolean): Promise<void> {
  let text = formatReference([payload.path])
  let wasTree = false
  if (payload.type === 'folder') {
    try {
      text = await buildDirTreeText(payload.path)
      wasTree = true
    } catch {
      text = formatReference([payload.path])
      wasTree = false
    }
  }
  if (onComposer && insertReferenceText(text)) {
    toastStore.show(wasTree ? '已追加目录树到输入框' : '已追加到输入框')
    return
  }
  void navigator.clipboard.writeText(text).then(
    () => { toastStore.show(wasTree ? '已复制目录树文本' : '已复制路径') },
    () => { toastStore.show('复制失败（剪贴板权限），请手动复制') },
  )
}

/** document 级拖拽监听（apply 注册一次）：capture 阶段接管，避免 composer 重复插文本。 */
function attachFileDragListeners(): () => void {
  const onDragOver = (e: DragEvent): void => {
    if (e.dataTransfer === null || !e.dataTransfer.types.includes(FILE_DRAG_MIME)) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'copy'
  }
  const onDrop = (e: DragEvent): void => {
    const payload = readDragPayload(e.dataTransfer)
    if (payload === null) return
    e.preventDefault()
    e.stopPropagation()
    const target = e.target
    const onComposer = target instanceof Element && target.closest('[data-composer-card]') !== null
    void deliverDroppedFile(payload, onComposer)
  }
  document.addEventListener('dragover', onDragOver, true)
  document.addEventListener('drop', onDrop, true)
  return () => {
    document.removeEventListener('dragover', onDragOver, true)
    document.removeEventListener('drop', onDrop, true)
  }
}

// ---------------------------------------------------------------------------
// 右键写操作菜单：树节点 / 搜索结果行 contextmenu → 重命名 / 复制路径 /
// 删除(回收站) / 新建文件 / 新建文件夹，全部 POST /dsh-anyfind/api/file/op；
// 破坏性操作 confirm；Ctrl 点选批量（≤128，超限提示）。
// 位置与目标集在模块级 store——浮层（搜索/文件视图）与侧栏文件 tab 共用一份。
// ---------------------------------------------------------------------------

interface MenuState {
  x: number
  y: number
  /** 重命名/复制/删除的目标（批量时多条）。 */
  paths: string[]
  /** 新建文件/文件夹的父目录（仅 canNew 时展示入口，对齐桌面端）。 */
  newDir: string
  /** 单选且是目录 → 给「新建文件/新建文件夹」（桌面端 ctxItemsForPaths 同条件）。 */
  canNew: boolean
}

const menuStore = {
  state: null as MenuState | null,
  listeners: new Set<() => void>(),
  open(state: MenuState): void {
    menuStore.state = state
    for (const l of menuStore.listeners) l()
  },
  close(): void {
    if (menuStore.state === null) return
    menuStore.state = null
    for (const l of menuStore.listeners) l()
  },
  subscribe(fn: () => void): () => void {
    menuStore.listeners.add(fn)
    return () => { menuStore.listeners.delete(fn) }
  },
  get(): MenuState | null {
    return menuStore.state
  },
}

/** 取父目录：`D:/a/b.txt` → `D:/a`；盘符根 `D:/a.txt` → `D:/`。 */
function fsParent(p: string): string {
  const norm = p.replace(/[\\/]+$/, '')
  const i = Math.max(norm.lastIndexOf('/'), norm.lastIndexOf('\\'))
  if (i < 0) return norm
  const parent = norm.slice(0, i)
  if (parent === '') return norm.startsWith('/') ? '/' : norm
  return /^[a-zA-Z]:$/.test(parent) ? `${parent}/` : parent
}

/** 提交 file/op：先本地校验（超限/缺参直接提示），再透传内核并聚合 results。 */
async function runFileOp(body: FileOpRequest, doneTip: string): Promise<void> {
  const invalid = validateFileOpRequest(body)
  if (invalid !== null) {
    toastStore.show(invalid)
    return
  }
  let res: Response
  try {
    res = await fetch('/dsh-anyfind/api/file/op', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
  } catch (e) {
    toastStore.show(errMsg(e))
    return
  }
  const data = await res.json().catch(() => null) as {
    ok?: boolean
    results?: Array<{ path: string; ok: boolean; error?: string }>
    error?: string
    down?: boolean
    endpoint?: string
  } | null
  if (data !== null && data.down === true) {
    toastStore.show(data.error ?? 'AnyFind 搜索服务未启动')
    return
  }
  if (data === null || !res.ok) {
    toastStore.show(data?.error ?? `HTTP ${res.status}`)
    return
  }
  const results = Array.isArray(data.results) ? data.results : []
  const failed = results.filter((r) => r.ok === false)
  if (failed.length > 0) {
    toastStore.show(`${failed.length}/${results.length} 项失败：${failed[0].error ?? '未知错误'}`)
  } else {
    toastStore.show(doneTip)
  }
  refreshAfterOp(body)
  // 全部成功才收尾选区/预览（部分失败时保留现场，便于重试）。
  if (failed.length === 0) {
    if (body.op === 'recycle' || body.op === 'rename') dropFromSelection(body.paths)
    settlePreviewAfterOp(body)
  }
}

/** 写操作后的预览收尾（桌面 afterOpRefresh 同型：删了就关，改名跟着改）。 */
function settlePreviewAfterOp(body: FileOpRequest): void {
  const st = files.state.preview
  if (st === null) return
  if (body.op === 'recycle' && body.paths.includes(st.path)) {
    filesClosePreview()
    return
  }
  if (body.op === 'rename' && body.paths[0] === st.path && typeof body.newName === 'string' && body.newName !== '') {
    const nextPath = joinFsPath(fsParent(st.path), body.newName)
    setFilesState((s) => (s.preview === null ? s : {
      ...s,
      preview: { ...s.preview, path: nextPath, name: body.newName as string },
    }))
  }
}

/** 写操作后只刷新已缓存过的相关目录（避免凭空发请求）。 */
function refreshAfterOp(body: FileOpRequest): void {
  const dirs = new Set<string>()
  if (body.op === 'newFile' || body.op === 'newDir') {
    for (const p of body.paths) dirs.add(p) // new* 的 paths 就是父目录
  } else {
    for (const p of body.paths) dirs.add(fsParent(p))
    if (typeof body.target === 'string' && body.target !== '') dirs.add(body.target)
  }
  for (const d of dirs) {
    if (files.state.lists[d] !== undefined) filesEnsureLoaded(d, true)
  }
}

/** 删除后把消失的路径从选区剔掉。 */
function dropFromSelection(paths: readonly string[]): void {
  setFilesState((s) => {
    const t = findTab(s, s.activeId)
    if (t === null) return s
    const gone = new Set(paths)
    const multi = t.multi.filter((p) => !gone.has(p))
    const selected = t.selected !== null && gone.has(t.selected) ? multi[multi.length - 1] ?? null : t.selected
    if (multi.length === t.multi.length && selected === t.selected) return s
    return { ...s, tabs: s.tabs.map((x) => (x.id === t.id ? { ...x, multi, selected } : x)) }
  })
}

function menuCopyPaths(paths: readonly string[]): void {
  const text = formatReference(paths)
  menuStore.close()
  if (text === '') return
  void navigator.clipboard.writeText(text).then(
    () => { toastStore.show(paths.length > 1 ? `已复制 ${paths.length} 条路径` : '已复制路径') },
    () => { toastStore.show('复制失败（剪贴板权限），请手动复制') },
  )
}

/** 与桌面端 renameFlow/newEntryFlow 同款名称预检（非法字符直接提示，不发请求）。 */
function invalidName(newName: string): boolean {
  if (/[\\/:*?"<>|]/.test(newName)) {
    toastStore.show('名称不能包含 \\ / : * ? " < > |')
    return true
  }
  return false
}

function menuRename(path: string): void {
  const oldName = fsBasename(path)
  const next = window.prompt('重命名', oldName)
  menuStore.close()
  if (next === null) return
  const newName = next.trim()
  if (newName === '') {
    toastStore.show('新名称不能为空')
    return
  }
  if (newName === oldName) return
  if (invalidName(newName)) return
  void runFileOp({ op: 'rename', paths: [path], target: fsParent(path), newName }, `已重命名为 ${newName}`)
}

function menuRecycle(paths: readonly string[]): void {
  // 文案对齐桌面端 deleteFlow（单条带路径、多条带计数，均标非永久删除）。
  const msg = paths.length === 1
    ? `确定把以下项移入回收站？（非永久删除）\n${paths[0]}`
    : `确定把以下 ${paths.length} 项移入回收站？（非永久删除）`
  const ok = window.confirm(msg)
  menuStore.close()
  if (!ok) return
  const label = paths.length === 1 ? fsBasename(paths[0]) : `${paths.length} 项`
  void runFileOp({ op: 'recycle', paths: [...paths] }, `已移入回收站：${label}`)
}

function menuNew(op: 'newFile' | 'newDir', dir: string): void {
  const next = window.prompt(op === 'newFile' ? `在 ${dir} 新建文件` : `在 ${dir} 新建文件夹`, '')
  menuStore.close()
  if (next === null) return
  const newName = next.trim()
  if (newName === '') {
    toastStore.show('名称不能为空')
    return
  }
  if (invalidName(newName)) return
  void runFileOp({ op, paths: [dir], newName },
    op === 'newFile' ? `已新建文件 ${newName}` : `已新建文件夹 ${newName}`)
}

interface ContextMenuLike {
  clientX: number
  clientY: number
  preventDefault(): void
}

/** 打开右键菜单：右键落在批量选区内 → 整批操作，否则收成单选该项。 */
function openContextMenu(e: ContextMenuLike, path: string, newDir: string, isDirTarget: boolean): void {
  e.preventDefault()
  const paths = filesContextSelect(path)
  menuStore.open({
    x: e.clientX,
    y: e.clientY,
    paths,
    newDir,
    canNew: isDirTarget && paths.length === 1,
  })
}

function FileContextMenu(): ReturnType<typeof createElement> | null {
  const m = useSyncExternalStore(menuStore.subscribe, menuStore.get)
  const open = m !== null

  useEffect(() => {
    if (!open) return
    const onClick = (e: MouseEvent): void => {
      const t = e.target
      if (t instanceof Element && t.closest('.rc-ctx') !== null) return
      menuStore.close()
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation() // 只关菜单，别把背后的浮层一起 Esc 掉
      menuStore.close()
    }
    document.addEventListener('click', onClick, true)
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('click', onClick, true)
      document.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  if (m === null) return null
  const single = m.paths.length === 1
  const left = Math.max(4, Math.min(m.x, window.innerWidth - 200))
  const top = Math.max(4, Math.min(m.y, window.innerHeight - 250))
  // 条目与顺序对齐桌面端 ctxItemsForPaths（重命名/复制路径/[新建仅目录]/删除）。
  return createElement('div', { className: 'rc-ctx', style: { left, top }, role: 'menu' },
    createElement('div', { className: 'rc-ctx-head' },
      single ? fsBasename(m.paths[0] ?? '') : `已选 ${m.paths.length} 项`),
    createElement('button', {
      role: 'menuitem',
      disabled: !single,
      title: single ? '重命名' : '批量时请逐项重命名',
      onClick: () => { if (single) menuRename(m.paths[0]) },
    }, '✎ 重命名'),
    createElement('button', { role: 'menuitem', onClick: () => { menuCopyPaths(m.paths) } }, '⧉ 复制路径'),
    m.canNew ? createElement('div', { className: 'rc-ctx-sep' }) : null,
    m.canNew ? createElement('button', { role: 'menuitem', onClick: () => { menuNew('newFile', m.newDir) } }, '＋ 新建文件') : null,
    m.canNew ? createElement('button', { role: 'menuitem', onClick: () => { menuNew('newDir', m.newDir) } }, '＋ 新建文件夹') : null,
    createElement('div', { className: 'rc-ctx-sep' }),
    createElement('button', { role: 'menuitem', className: 'danger', onClick: () => { menuRecycle(m.paths) } },
      single ? '🗑 删除（回收站）' : `🗑 删除 ${m.paths.length} 项（回收站）`),
  )
}

function FileTreePanel(): ReturnType<typeof createElement> {
  const st = useSyncExternalStore(filesSubscribe, filesGetSnapshot)
  const toast = useSyncExternalStore(toastStore.subscribe, toastStore.get)
  const tab = findTab(st, st.activeId)
  const current = tab !== null ? currentDir(tab) : null
  const list = current !== null ? st.lists[current] : undefined
  const [draft, setDraft] = useState('')

  useEffect(() => { setDraft(current ?? '') }, [current])
  useEffect(() => { if (current !== null) filesEnsureLoaded(current) }, [current])

  const rows = current !== null && list?.status === 'ready' ? buildFileRows(st, current) : []
  const truncatedNote = list?.status === 'ready' && list.truncated
    ? `目录条目超过上限已截断，只显示前 ${list.entries.length} 条（噪声目录已隐藏）——进入子目录可继续浏览`
    : ''

  const body = tab === null
    ? createElement('div', { className: 'rc-fempty' },
        createElement(FilesIcon, { size: 26 }),
        createElement('div', null, '还没有文件标签'),
        createElement('button', { className: 'rc-btn rc-btn-pri', onClick: () => { filesNewTab() } }, '＋ 新建标签'),
      )
    : list === undefined || list.status === 'loading'
      ? createElement('div', { className: 'rc-loading' },
          createElement('div', { className: 'rc-loading-t' }, createElement('span', { className: 'rc-spin' }), '加载目录…'),
          createElement('div', { className: 'rc-skel' },
            createElement('div', { className: 'rc-skel-row' }),
            createElement('div', { className: 'rc-skel-row' }),
            createElement('div', { className: 'rc-skel-row' }),
          ),
        )
      : list.status === 'error'
        ? createElement('div', { className: 'rc-guide' },
            createElement('div', { className: 'rc-banner rc-banner-err' }, list.error),
            createElement('button', { className: 'rc-btn rc-btn-pri', style: { alignSelf: 'flex-start' },
              onClick: () => { if (current !== null) filesEnsureLoaded(current, true) } }, '⟳ 重试'),
          )
        : createElement('div', { className: 'rc-tree' }, ...rows.map((row) => {
            if (row.kind !== 'entry') {
              return createElement('div', { key: row.key, className: 'rc-frow muted', style: { paddingLeft: 18 + row.depth * 14 } },
                createElement('span', { className: 'rc-fname' }, row.name))
            }
            const selected = tab.multi.includes(row.path) || tab.selected === row.path
            return createElement('div', {
              key: row.key,
              className: `rc-frow${row.isDir ? ' dir' : ''}${selected ? ' sel' : ''}`,
              style: { paddingLeft: 10 + row.depth * 14 },
              title: row.path,
              draggable: true,
              onDragStart: (e: DragStartLike) => {
                startFileDrag(e, { path: row.path, name: row.name, type: row.isDir ? 'folder' : 'file' })
              },
              onClick: (e: { ctrlKey: boolean; metaKey: boolean }) => {
                if (e.ctrlKey || e.metaKey) {
                  filesToggleMulti(row.path)
                  return
                }
                row.isDir ? filesToggleExpand(row.path) : filesSelect(row.path)
              },
              onDoubleClick: () => { if (row.isDir) filesNavigate(row.path) },
              onContextMenu: (e: ContextMenuLike) => {
                openContextMenu(e, row.path, row.isDir ? row.path : (current ?? fsParent(row.path)), row.isDir)
              },
            },
              createElement('span', { className: 'rc-fico' }, row.isDir ? (row.expanded ? '▾' : '▸') : '·'),
              createElement('span', { className: 'rc-fname' }, row.name),
              row.isDir ? null : createElement('span', { className: 'rc-fmeta w' }, fmtBytes(row.size)),
              createElement('span', { className: 'rc-fmeta d' }, fmtEntryTime(row.mtime)),
              createElement('span', {
                className: 'rc-fops',
                onClick: (e: { stopPropagation(): void }) => { e.stopPropagation() },
              },
                createElement('button', { title: '预览',
                  onClick: () => { filesOpenPreview(row.path, row.name, row.isDir) } }, '预览'),
                createElement('button', { title: '引用到输入框',
                  onClick: () => { referenceOrCopy(row.path) } }, '引用'),
              ),
            )
          }))

  return createElement('div', { className: 'rc-panel' },
    createElement(StyleTag),
    createElement('div', { className: 'rc-ftabs' },
      ...st.tabs.map((t, i) => {
        const active = t.id === st.activeId
        const label = fsBasename(currentDir(t))
        return createElement('div', {
          key: t.id,
          className: `rc-ftab${active ? ' active' : ''}`,
          title: currentDir(t),
          onClick: () => { filesActivate(t.id) },
        },
          createElement('span', { className: 'rc-ftab-name' }, label),
          createElement('span', { className: 'rc-ftab-ops' },
            createElement('button', { title: '左移', disabled: i === 0,
              onClick: (e: { stopPropagation(): void }) => { e.stopPropagation(); filesMoveTab(t.id, -1) } }, '‹'),
            createElement('button', { title: '右移', disabled: i === st.tabs.length - 1,
              onClick: (e: { stopPropagation(): void }) => { e.stopPropagation(); filesMoveTab(t.id, 1) } }, '›'),
            createElement('button', { title: '关闭标签',
              onClick: (e: { stopPropagation(): void }) => { e.stopPropagation(); filesCloseTab(t.id) } }, '×'),
          ),
        )
      }),
      createElement('button', { className: 'rc-ftab-new', title: '新建标签',
        onClick: () => { filesNewTab(tab !== null ? currentDir(tab) : undefined) } }, '＋'),
    ),
    st.preview !== null
      ? createElement(FilePreview)
      : createElement('div', { className: 'rc-main' },
          createElement('div', { className: 'rc-fbar' },
            createElement('button', { className: 'rc-btn', title: '后退', disabled: (tab?.history.length ?? 0) <= 1,
              onClick: filesBack }, '←'),
            createElement('input', {
              className: 'rc-fpath',
              value: draft,
              spellCheck: false,
              placeholder: '绝对路径，如 D:/coding…',
              onChange: (e: { target: { value: string } }) => { setDraft(e.target.value) },
              onKeyDown: (e: { key: string; preventDefault(): void }) => {
                if (e.key !== 'Enter') return
                e.preventDefault()
                if (draft.trim() !== '') filesNavigate(draft)
              },
            }),
            createElement('button', { className: 'rc-btn', title: '刷新', disabled: current === null,
              onClick: () => { if (current !== null) filesEnsureLoaded(current, true) } }, '⟳'),
          ),
          truncatedNote !== '' ? createElement('div', { className: 'rc-banner rc-banner-info' }, truncatedNote) : null,
          body,
        ),
    st.preview === null && tab !== null && (tab.selected !== null || tab.multi.length > 0)
      ? createElement('div', { className: 'rc-ffoot' },
          createElement('span', { className: 'rc-fsel', title: tab.multi.join('\n') },
            tab.multi.length > 1 ? `已选 ${tab.multi.length} 项` : (tab.selected ?? '')),
          createElement('button', { className: 'rc-btn',
            onClick: () => { menuCopyPaths(tab.multi.length > 0 ? tab.multi : [tab.selected ?? '']) } },
            '复制路径'),
        )
      : null,
    toast !== '' ? createElement('div', { className: 'rc-toast' }, toast) : null,
    createElement(FileContextMenu),
  )
}

// ---------------------------------------------------------------------------
// Spotlight 浮层（shell.overlay 机制，与 dsh-hippo MemoryOverlay 同套）
// ---------------------------------------------------------------------------

function AnyfindOverlay(): ReturnType<typeof createElement> | null {
  const open = useSyncExternalStore(overlayStore.subscribe, () => overlayStore.open)
  const view = useSyncExternalStore(overlayStore.subscribe, () => overlayStore.view)

  // 全局热键：Ctrl+K 呼出/关闭（宿主核心包与已装插件均无此组合占用——已排查
  // dsh-fresh packages/apps、dsh-better-sidebar、dsh-hippo、dsh-termfleet 源码）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
        if (e.isComposing) return
        e.preventDefault()
        overlayStore.toggle()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => { window.removeEventListener('keydown', onKey) }
  }, [])

  // 打开后聚焦搜索框；Esc 关闭
  useEffect(() => {
    if (!open) return
    const t = setTimeout(() => { document.querySelector<HTMLInputElement>('.rc-modal .rc-input')?.focus() }, 60)
    const onKey = (e: KeyboardEvent): void => { if (e.key === 'Escape') overlayStore.close() }
    window.addEventListener('keydown', onKey)
    return () => { clearTimeout(t); window.removeEventListener('keydown', onKey) }
  }, [open])

  if (!open) return null
  return createElement('div', {
    className: 'rc-mask',
    onClick: (e: { target: unknown; currentTarget: unknown }) => { if (e.target === e.currentTarget) overlayStore.close() },
  },
    createElement('div', { className: 'rc-modal', role: 'dialog', 'aria-label': view === 'files' ? '文件' : '统一搜索' },
      createElement('div', { className: 'rc-ovsw', role: 'tablist', 'aria-label': '视图切换' },
        createElement('button', {
          className: `rc-ovsw-b${view === 'search' ? ' active' : ''}`,
          role: 'tab',
          'aria-selected': view === 'search',
          onClick: () => { overlayStore.setView('search') },
        }, '搜索'),
        createElement('button', {
          className: `rc-ovsw-b${view === 'files' ? ' active' : ''}`,
          role: 'tab',
          'aria-selected': view === 'files',
          onClick: () => { overlayStore.setView('files') },
        }, '文件'),
      ),
      view === 'search' ? createElement(AnyfindPanel) : createElement(FileTreePanel),
    ),
  )
}

/** 占位 tab：侧边卡片/右栏入口点击 → 呼出浮层。
 *  better-sidebar 路径：挂载后 show 浮层 + closeTab(tab.id) 自移除（即开即关，
 *  底部工作台不留痕；reload 恢复挂载只自闭不弹层，5s 启动守卫区分）。
 *  官方直注册路径无已知 closeTab API，退化为提示卡 + 重新打开按钮。 */
let bootAt = 0

function AnyfindTabPlaceholder(props: TabComponentPropsLike): ReturnType<typeof createElement> {
  useEffect(() => {
    const isRestore = Date.now() - bootAt <= 5000
    if (!isRestore) overlayStore.show()
    const tabId = props.tab?.id
    if (tabId !== undefined && betterSidebarSvc !== null && typeof betterSidebarSvc.closeTab === 'function') {
      betterSidebarSvc.closeTab(tabId, props.scope)
      // 工作台因本次打开而展开且已无剩余 tab 时收合——点卡片只见浮层，底部不留痕；
      // 用户原本开着工作台（有其他 tab）则不动它的开合状态。
      const store = props.store
      const st = store?.getSnapshot().state
      if (store !== undefined && st?.bottomOpen === true && countRemainingTabs(st.bottomSplits) === 0) {
        store.reduce((s) => ({ ...s, bottomOpen: false }))
      }
    }
  }, [])
  return createElement('div', { className: 'rc-panel' },
    createElement(StyleTag),
    createElement('div', { className: 'rc-ph' },
      createElement(AnyfindMark, { size: 28 }),
      createElement('div', { className: 'rc-ph-t' }, '搜索以浮层打开'),
      createElement('div', null, '按 ', createElement('kbd', null, 'Ctrl+K'), ' 随时呼出 / Esc 关闭'),
      createElement('button', { className: 'rc-btn rc-btn-pri', onClick: () => { overlayStore.show() } }, '重新打开搜索'),
    ),
  )
}

function AnyfindTabTitle(): ReturnType<typeof createElement> {
  return createElement('span', {
    style: { display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--dsw-alias-label-primary, inherit)' },
  }, createElement(AnyfindMark, { size: 14 }), '搜索')
}

function AnyfindGuideIcon(props?: { size?: number }): ReturnType<typeof createElement> {
  return createElement(AnyfindMark, { size: props?.size ?? 22 })
}

// ---------------------------------------------------------------------------
// 会话头胶囊：conversation.session.header.utilities（session scope，组件自动收
// props.inputActions / props.useInput 等标准 props）。点击打开 AnyfindOverlay；
// props.inputActions 存进模块级 store（跟随当前会话，换会话由 effect 重挂）。
// 形态参照 dsh-workspace-explorer 的 HeaderAction（功能名 + 图标胶囊）。
// ---------------------------------------------------------------------------

interface CapsuleProps {
  inputActions?: { setDraft(text: string): void } | undefined
  useInput?: ((selector: (s: unknown) => unknown) => unknown) | undefined
}

function AnyfindCapsule(props: CapsuleProps): ReturnType<typeof createElement> {
  const hasActions = typeof props.inputActions?.setDraft === 'function'
  const hasInput = typeof props.useInput === 'function'
  const input = hasInput ? props.useInput?.((s: unknown) => s) : undefined
  const draft = input !== null && typeof input === 'object' && typeof (input as { draft?: unknown }).draft === 'string'
    ? (input as { draft: string }).draft
    : ''
  const draftRef = useRef('')
  useEffect(() => { draftRef.current = draft }, [draft])
  const open = useSyncExternalStore(overlayStore.subscribe, () => overlayStore.open)

  // 只有 inputActions 与 useInput 齐备才架桥：缺 useInput 就不知道当前 draft，
  // 盲用 setDraft 会清空输入框——此时保持无桥，「引用」走复制降级。
  useEffect(() => {
    if (!hasActions || !hasInput) return
    const actions = props.inputActions
    if (actions === undefined) return
    const bridge: DraftBridge = {
      insert(text: string): void {
        const cur = draftRef.current
        const sep = cur === '' || cur.endsWith('\n') ? '' : '\n'
        actions.setDraft(cur + sep + text)
      },
    }
    setDraftBridge(bridge)
    return () => { clearDraftBridge(bridge) }
  }, [props.inputActions, hasActions, hasInput])

  return createElement('button', {
    className: `rc-cap${open ? ' on' : ''}`,
    type: 'button',
    title: 'AnyFind：统一搜索 / 文件预览 / @ 文件引用（Ctrl+K 亦可呼出）',
    onClick: () => { overlayStore.show() },
  }, createElement(AnyfindMark, { size: 13 }), 'AnyFind')
}

// ---------------------------------------------------------------------------
// @ 触发源 anyfind-files：按输入查 /dsh-anyfind/api/search?q=…&kinds=file&limit=50，
// 出路径候选；onPick 返 reference 插入（ref=绝对路径，codec 原样序列化）。
// ---------------------------------------------------------------------------

async function searchFileCandidates(query: string, signal?: AbortSignal): Promise<InputTriggerCandidate[]> {
  const params = new URLSearchParams({ q: query, kinds: 'file', limit: '50' })
  const res = await fetch(`/dsh-anyfind/api/search?${params}`, { cache: 'no-store', signal })
  const body = await res.json().catch(() => null) as (SearchResponse & { down?: boolean; error?: string }) | null
  if (body === null || !res.ok) throw new Error(body?.error ?? `HTTP ${res.status}`)
  const hits = (body.groups.file ?? []).filter((h) => typeof h.path === 'string' && h.path !== '')
  return hits.map((h) => {
    const path = h.path ?? ''
    // 内核 file 组不区分目录：无扩展名按目录呈现（启发式）。
    const folder = extOf(path) === ''
    const snippet = h.snippet !== null && h.snippet !== '' ? stripMark(h.snippet).replace(/\s+/g, ' ').trim() : ''
    return {
      name: path,
      value: path,
      icon: folder ? 'folder' as const : 'file' as const,
      ...(snippet !== '' ? { description: snippet.slice(0, 140) } : {}),
    } satisfies InputTriggerCandidate
  })
}

function registerAnyfindTrigger(ctx: Context): void {
  const triggers = ctx.inputTriggers
  if (triggers === undefined || typeof triggers.registerSource !== 'function') {
    console.warn('[dsh-anyfind] 宿主未提供 inputTriggers 服务，@ 文件引用触发源跳过注册')
    return
  }
  const source: InputTriggerSource = {
    trigger: '@',
    name: 'anyfind-files',
    order: 10,
    async candidates(_session, req): Promise<readonly InputTriggerCandidate[]> {
      const query = req.query.trim()
      if (query === '') return []
      try {
        return await searchFileCandidates(query, req.signal)
      } catch {
        return []
      }
    },
    onPick(pick) {
      const path = pick.candidate.value ?? pick.candidate.name
      if (path === '') return undefined
      return {
        insert: {
          // 必须等于注册名：提交时 serializeReference 按 source 名在 roster 里找 codec
          // （input-trigger lib/client.js：roster.all().find(s => s.name === source)）。
          source: 'anyfind-files',
          ref: path,
          label: fsBasename(path),
          appearance: pick.candidate.icon === 'folder' ? 'folder' : 'file',
          clipboardText: path,
        },
      }
    },
    codec: {
      clipboardText: (ref) => ref,
      serialize: (ref) => Promise.resolve(ref),
    },
  }
  ctx.effect(() => triggers.registerSource(source), 'dsh-anyfind: @ trigger')
}

export function apply(ctx: Context): void {
  const slots: unknown = ctx.slots
  if (
    slots === null || typeof slots !== 'object'
    || typeof (slots as { inject?: unknown }).inject !== 'function'
    || typeof (slots as { register?: unknown }).register !== 'function'
  ) {
    console.warn('[dsh-anyfind] 宿主未提供可用的 slots 服务（需要 @deepseek-ai/dsh-client-ui-renderer），搜索浮层跳过挂载')
    return
  }
  const dyn = ctx.slots as unknown as DynamicSlots
  bootAt = Date.now()

  // 主形态：shell.overlay 浮层（Ctrl+K 呼出；dsh-hippo 同套机制）
  ctx.slots.inject('shell.overlay', () => ctx.slots.register(
    { name: 'shell.overlay', id: 'anyfind-spotlight', order: 48 },
    () => createElement(AnyfindOverlay),
  ))

  // 会话头胶囊：点击开浮层；组件把 props.inputActions 存进模块级桥（跟随当前会话）。
  ctx.slots.inject('conversation.session.header.utilities', () => ctx.slots.register(
    { name: 'conversation.session.header.utilities', id: 'anyfind-capsule', order: 30, label: () => 'AnyFind' },
    (props: CapsuleProps) => createElement(AnyfindCapsule, props),
  ))

  // @ 触发源（export inject 已声明 inputTriggers）与 document 级拖拽落框。
  registerAnyfindTrigger(ctx)
  ctx.effect(() => attachFileDragListeners(), 'dsh-anyfind: file dnd')

  // 入口保留但点击改为呼出浮层：占位 tab（挂载即 show + 提示卡）。双路径互斥，kind 固定 'anyfind'。
  const registerDirectTab = (tabs: SidebarRightTabsRegistry): (() => void) => {
    const disposeType = tabs.register({
      id: 'dsh-anyfind',
      kind: 'anyfind',
      keepMounted: false,
      title: () => '搜索',
      guide: [{
        id: 'anyfind',
        order: 62,
        title: () => '搜索',
        description: () => '统一搜索（浮层）：文件 / AI 会话 / 技能 / 记忆（Ctrl+K）',
        icon: AnyfindGuideIcon,
      }],
    })
    const disposeBody = dyn.inject('sidebar.right.pane.tab', () => dyn.register(
      { name: 'sidebar.right.pane.tab', key: 'dsh-anyfind', registrant: 'dsh-anyfind' },
      (props: TabComponentPropsLike) => createElement(AnyfindTabPlaceholder, props),
    ))
    const disposeTitle = dyn.inject('sidebar.right.pane.tab.title', () => dyn.register(
      { name: 'sidebar.right.pane.tab.title', key: 'dsh-anyfind', registrant: 'dsh-anyfind' },
      AnyfindTabTitle,
    ))
    return () => { disposeBody(); disposeTitle(); disposeType() }
  }

  let tabsRegistry: SidebarRightTabsRegistry | null = null
  const syncDirectTab = (): void => {
    if (betterSidebarSvc !== null) {
      if (disposeDirectTab !== null) { disposeDirectTab(); disposeDirectTab = null }
      return
    }
    if (tabsRegistry !== null && disposeDirectTab === null) {
      disposeDirectTab = registerDirectTab(tabsRegistry)
    }
  }

  ctx.inject(['sidebarRightTabs'], (sc) => {
    tabsRegistry = (sc.get('sidebarRightTabs') as SidebarRightTabsRegistry | undefined) ?? null
    syncDirectTab()
    return () => {
      tabsRegistry = null
      if (disposeDirectTab !== null) { disposeDirectTab(); disposeDirectTab = null }
      syncDirectTab()
    }
  })

  ctx.inject(['betterSidebar'], (sc) => {
    betterSidebarSvc = (sc.get('betterSidebar') as BetterSidebarLike | undefined) ?? null
    let disposeBs: (() => void) | null = null
    let disposeBsFiles: (() => void) | null = null
    if (betterSidebarSvc !== null) {
      // 先让位再注册：registerTab 内部会立即向 native sidebarRight 注册 kind，
      // 若直注册 tab 仍在（sidebarRightTabs 服务先到的时序），会撞
      // 「tab kind "anyfind" is already registered」——必须在它之前撤掉直注册。
      if (disposeDirectTab !== null) { disposeDirectTab(); disposeDirectTab = null }
      disposeBs = betterSidebarSvc.registerTab({
        id: 'anyfind',
        title: () => '搜索',
        description: () => '统一搜索（浮层）：文件 / AI 会话 / 技能 / 记忆（Ctrl+K）',
        icon: (size) => AnyfindGuideIcon({ size }),
        order: 62,
        single: true,
        component: (props) => createElement(AnyfindTabPlaceholder, props as TabComponentPropsLike),
      })
      // 文件 tab：与浮层「文件」视图共享同一模块级 store（标签/展开/选中/缓存互通）。
      disposeBsFiles = betterSidebarSvc.registerTab({
        id: 'anyfind-files',
        title: () => '文件',
        description: () => '文件树（多标签）：目录浏览 / 噪声过滤 / 文件选中',
        icon: (size) => FilesIcon({ size }),
        order: 63,
        single: true,
        component: () => createElement(FileTreePanel),
      })
    }
    syncDirectTab()
    return () => {
      if (disposeBs !== null) disposeBs()
      if (disposeBsFiles !== null) disposeBsFiles()
      betterSidebarSvc = null
      syncDirectTab()
    }
  })
}
