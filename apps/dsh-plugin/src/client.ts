/**
 * dsh-recall 浏览器半：Spotlight 式居中搜索浮层。
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
 * 数据通路：同源 fetch /dsh-recall/api/*（host 半代理到内核，避开 CORS）。
 * @module dsh-recall/client
 */

import { createElement, useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
// 0.2.0-rc.2：一方客户端插件直接收 cordis Context。
import type { Context } from '@deepseek-ai/cordis'
// Type-only: ctx.slots（SlotRegistry 服务）由 ui-renderer 的 cordis Context 合并提供。
import type {} from '@deepseek-ai/dsh-client-ui-renderer/client'
import type { Hit, SearchResponse } from './core.ts'
import { KIND_LABEL, KINDS, escapeWithMark, fmtMtime } from './core.ts'

export const inject = ['slots']

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
  subscribe(fn: () => void): () => void {
    overlayStore.listeners.add(fn)
    return () => { overlayStore.listeners.delete(fn) }
  },
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

@media (prefers-reduced-motion: reduce) { .rc-hit, .rc-syn, .rc-mask, .rc-modal, .rc-toast { animation: none !important; }
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
  const res = await fetch('/dsh-recall/api/health', { cache: 'no-store' })
  const body = await res.json() as HealthState & { down?: boolean; error?: string }
  if (!res.ok || body.down === true) throw coreDown(body, `HTTP ${res.status}`)
  return body
}

/** 一次拉全量 groups（不带 kinds）——kind 切换纯客户端过滤，零请求。 */
async function getSearch(q: string): Promise<SearchResponse> {
  const url = new URLSearchParams({ q, limit: '10' })
  const res = await fetch(`/dsh-recall/api/search?${url.toString()}`, { cache: 'no-store' })
  const body = await res.json() as SearchResponse & { down?: boolean; error?: string; endpoint?: string }
  if (body.down === true) throw coreDown(body, '内核未启动')
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`)
  return body
}

async function postResume(hit: Hit): Promise<string> {
  const res = await fetch('/dsh-recall/api/resume', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: hit.agent, session_id: hit.session_id, cwd: hit.cwd }),
  })
  const body = await res.json() as { ok: boolean; command?: string; error?: string; down?: boolean; endpoint?: string }
  if (body.down === true) throw coreDown(body, '内核未启动')
  if (!res.ok || !body.ok || typeof body.command !== 'string') throw new Error(body.error ?? `HTTP ${res.status}`)
  return body.command
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

function RecallMark({ size = 16 }: { size?: number }): ReturnType<typeof createElement> {
  return createElement('svg', { width: size, height: size, viewBox: '0 0 24 24', 'aria-hidden': true, fill: 'none', stroke: 'currentColor', strokeWidth: 2.4, strokeLinecap: 'round' },
    createElement('circle', { cx: 10.5, cy: 10.5, r: 6.5 }),
    createElement('line', { x1: 15.5, y1: 15.5, x2: 21, y2: 21 }),
  )
}

/** 命中高亮：escape 后仅恢复 <mark>（内核契约高亮标签），防索引内容注入。 */
function Snippet({ text }: { text: string }): ReturnType<typeof createElement> {
  return createElement('div', { className: 'rc-hit-s', dangerouslySetInnerHTML: { __html: escapeWithMark(text) } })
}

function HitRow({ hit, focused, cmd, busyKey, onResume, onCopy, onDown }: {
  hit: Hit
  focused: boolean
  cmd: string | null
  busyKey: boolean
  onResume: (hit: Hit) => void
  onCopy: (text: string, tip: string) => void
  onDown: (endpoint?: string) => void
}): ReturnType<typeof createElement> {
  const [err, setErr] = useState<string | null>(null)
  const rowRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (focused) rowRef.current?.scrollIntoView({ block: 'nearest' })
  }, [focused])

  const meta = [hit.agent, hit.project, fmtMtime(hit.mtime)].filter((x) => x !== null && x !== '')
  return createElement('div', { className: `rc-hit${focused ? ' focused' : ''}`, ref: rowRef },
    createElement('div', { className: 'rc-hit-t' }, hit.title ?? hit.path ?? '(无标题)'),
    meta.length > 0 ? createElement('div', { className: 'rc-hit-meta' }, ...meta.map((m, i) => createElement('span', { key: i }, m))) : null,
    hit.snippet !== null && hit.snippet !== '' ? createElement(Snippet, { text: hit.snippet }) : null,
    hit.path !== null && hit.path !== hit.title ? createElement('div', { className: 'rc-hit-path' }, hit.path) : null,
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
function DownGuide({ detail, endpoint, onRetry }: { detail: string; endpoint: string; onRetry: () => void }): ReturnType<typeof createElement> {
  let port = '8790'
  try {
    port = new URL(endpoint).port || '8790'
  } catch { /* 展示默认值即可 */ }
  return createElement('div', { className: 'rc-guide' },
    createElement('div', { className: 'rc-guide-t' }, 'AnyFind 内核未启动'),
    createElement('div', { className: 'rc-banner rc-banner-info' },
      `面板与 agent 工具都依赖本地内核（${endpoint}）。启动后此处自动可用：`),
    createElement('div', { className: 'rc-cmd' }, `anyfindd --port ${port}`),
    createElement('div', { className: 'rc-banner rc-banner-info' },
      '内核监听其他地址？在下方「内核地址」改成实际地址后重试。'),
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

function RecallPanel(): ReturnType<typeof createElement> {
  const [health, setHealth] = useState<HealthState | null>(null)
  const [downDetail, setDownDetail] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [activeKind, setActiveKind] = useState('')
  const [resp, setResp] = useState<SearchResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [synOpen, setSynOpen] = useState(false)
  const [endpointDraft, setEndpointDraft] = useState('')
  const [savedTip, setSavedTip] = useState(false)
  const [sel, setSel] = useState(-1)
  const [cmds, setCmds] = useState<Record<string, string>>({})
  const [resumeBusy, setResumeBusy] = useState<string | null>(null)
  const [toast, setToast] = useState('')
  const seq = useRef(0)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const hlRef = useRef<HTMLDivElement | null>(null)

  const checkHealth = (): void => {
    void getHealth().then(
      (h) => { setHealth(h); setDownDetail(null); setEndpointDraft(h.endpoint ?? '') },
      (e: unknown) => {
        setHealth(null)
        setDownDetail(e instanceof Error ? e.message : String(e))
        if (e instanceof CoreDown && e.endpoint !== undefined) setEndpointDraft(e.endpoint)
      },
    )
  }
  useEffect(checkHealth, [])

  useEffect(() => {
    if (toast === '') return
    const t = setTimeout(() => { setToast('') }, 2200)
    return () => { clearTimeout(t) }
  }, [toast])

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
          if (e.endpoint !== undefined) setEndpointDraft(e.endpoint)
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

  const saveEndpoint = (): void => {
    void fetch('/dsh-recall/api/config', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ endpoint: endpointDraft }),
    }).then(async (res) => {
      if (res.ok) {
        setSavedTip(true)
        setTimeout(() => { setSavedTip(false) }, 2000)
        checkHealth()
      } else {
        const body = await res.json() as { error?: string }
        setError(body.error ?? `HTTP ${res.status}`)
      }
    }).catch(() => {})
  }

  const copyText = (text: string, tip: string): void => {
    void navigator.clipboard.writeText(text).then(
      () => { setToast(tip) },
      () => { setToast('复制失败（剪贴板权限），请手动复制') },
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
            if (e.endpoint !== undefined) setEndpointDraft(e.endpoint)
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
        ? createElement(DownGuide, { detail: downDetail ?? '', endpoint: endpointDraft || 'http://127.0.0.1:8790', onRetry: checkHealth })
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
            onDown: (ep) => {
              setHealth(null)
              setDownDetail('内核连接中断')
              if (ep !== undefined) setEndpointDraft(ep)
            },
          })
        }),
      )) : []),
    ),
    toast !== '' ? createElement('div', { className: 'rc-toast' }, toast) : null,
    createElement('div', { className: 'rc-foot', title: '统一搜索内核地址（默认 http://127.0.0.1:8790）' },
      createElement('span', { style: { fontSize: 11, color: 'var(--rc-mut)', flex: 'none' } }, '内核地址'),
      createElement('input', { value: endpointDraft, placeholder: 'http://127.0.0.1:8790', onChange: (e: { target: { value: string } }) => { setEndpointDraft(e.target.value) } }),
      createElement('button', { className: 'rc-btn', onClick: saveEndpoint }, savedTip ? '✓ 已保存' : '保存'),
    ),
  )
}

// ---------------------------------------------------------------------------
// Spotlight 浮层（shell.overlay 机制，与 dsh-hippo MemoryOverlay 同套）
// ---------------------------------------------------------------------------

function RecallOverlay(): ReturnType<typeof createElement> | null {
  const open = useSyncExternalStore(overlayStore.subscribe, () => overlayStore.open)

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
    createElement('div', { className: 'rc-modal', role: 'dialog', 'aria-label': '统一搜索' },
      createElement(RecallPanel),
    ),
  )
}

/** 占位 tab：侧边卡片/右栏入口点击 → 呼出浮层。
 *  better-sidebar 路径：挂载后 show 浮层 + closeTab(tab.id) 自移除（即开即关，
 *  底部工作台不留痕；reload 恢复挂载只自闭不弹层，5s 启动守卫区分）。
 *  官方直注册路径无已知 closeTab API，退化为提示卡 + 重新打开按钮。 */
let bootAt = 0

function RecallTabPlaceholder(props: TabComponentPropsLike): ReturnType<typeof createElement> {
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
      createElement(RecallMark, { size: 28 }),
      createElement('div', { className: 'rc-ph-t' }, '搜索以浮层打开'),
      createElement('div', null, '按 ', createElement('kbd', null, 'Ctrl+K'), ' 随时呼出 / Esc 关闭'),
      createElement('button', { className: 'rc-btn rc-btn-pri', onClick: () => { overlayStore.show() } }, '重新打开搜索'),
    ),
  )
}

function RecallTabTitle(): ReturnType<typeof createElement> {
  return createElement('span', {
    style: { display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--dsw-alias-label-primary, inherit)' },
  }, createElement(RecallMark, { size: 14 }), '搜索')
}

function RecallGuideIcon(props?: { size?: number }): ReturnType<typeof createElement> {
  return createElement(RecallMark, { size: props?.size ?? 22 })
}

export function apply(ctx: Context): void {
  const slots: unknown = ctx.slots
  if (
    slots === null || typeof slots !== 'object'
    || typeof (slots as { inject?: unknown }).inject !== 'function'
    || typeof (slots as { register?: unknown }).register !== 'function'
  ) {
    console.warn('[dsh-recall] 宿主未提供可用的 slots 服务（需要 @deepseek-ai/dsh-client-ui-renderer），搜索浮层跳过挂载')
    return
  }
  const dyn = ctx.slots as unknown as DynamicSlots
  bootAt = Date.now()

  // 主形态：shell.overlay 浮层（Ctrl+K 呼出；dsh-hippo 同套机制）
  ctx.slots.inject('shell.overlay', () => ctx.slots.register(
    { name: 'shell.overlay', id: 'recall-spotlight', order: 48 },
    () => createElement(RecallOverlay),
  ))

  // 入口保留但点击改为呼出浮层：占位 tab（挂载即 show + 提示卡）。双路径互斥，kind 固定 'recall'。
  const registerDirectTab = (tabs: SidebarRightTabsRegistry): (() => void) => {
    const disposeType = tabs.register({
      id: 'dsh-recall',
      kind: 'recall',
      keepMounted: false,
      title: () => '搜索',
      guide: [{
        id: 'recall',
        order: 62,
        title: () => '搜索',
        description: () => '统一搜索（浮层）：文件 / AI 会话 / 技能 / 记忆（Ctrl+K）',
        icon: RecallGuideIcon,
      }],
    })
    const disposeBody = dyn.inject('sidebar.right.pane.tab', () => dyn.register(
      { name: 'sidebar.right.pane.tab', key: 'dsh-recall', registrant: 'dsh-recall' },
      (props: TabComponentPropsLike) => createElement(RecallTabPlaceholder, props),
    ))
    const disposeTitle = dyn.inject('sidebar.right.pane.tab.title', () => dyn.register(
      { name: 'sidebar.right.pane.tab.title', key: 'dsh-recall', registrant: 'dsh-recall' },
      RecallTabTitle,
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
    if (betterSidebarSvc !== null) {
      disposeBs = betterSidebarSvc.registerTab({
        id: 'recall',
        title: () => '搜索',
        description: () => '统一搜索（浮层）：文件 / AI 会话 / 技能 / 记忆（Ctrl+K）',
        icon: (size) => RecallGuideIcon({ size }),
        order: 62,
        single: true,
        component: (props) => createElement(RecallTabPlaceholder, props as TabComponentPropsLike),
      })
    }
    syncDirectTab()
    return () => {
      if (disposeBs !== null) disposeBs()
      betterSidebarSvc = null
      syncDirectTab()
    }
  })
}
