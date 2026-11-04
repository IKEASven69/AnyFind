/**
 * 统一搜索内核（filefind）HTTP 客户端 + 结果格式化。
 * host 半的代理路由、anyfind_search 工具、/anyfind 斜杠命令共用这一层；
 * 契约对齐 docs/api-contract.md v0.1（端点路径/参数名/响应字段名固定）。
 * @module dsh-anyfind/core
 */

/** 契约：GET /api/health 响应。 */
export interface HealthResponse {
  ok: boolean
  version?: string
  index?: { files?: number; sessions?: number; skills?: number; memories?: number }
  indexing?: boolean
}

/** 契约：统一 Hit 结构（各 kind 共用，不适用字段为 null）。 */
export interface Hit {
  kind: string
  title: string | null
  path: string | null
  snippet: string | null
  mtime: string | null
  agent: string | null
  project: string | null
  session_id: string | null
  cwd: string | null
  score?: number
}

/** 契约：GET /api/search 响应。 */
export interface SearchResponse {
  q: string
  took_ms?: number
  groups: Record<string, Hit[]>
  truncated?: Record<string, boolean>
}

/** 契约：POST /api/resume 响应。 */
export interface ResumeResponse {
  ok: boolean
  command?: string
  error?: string
}

export const KINDS = ['file', 'content', 'session', 'skill', 'memory'] as const

export const KIND_LABEL: Record<string, string> = {
  file: '文件',
  content: '内容',
  session: '会话',
  skill: '技能',
  memory: '记忆',
}

const DEFAULT_ENDPOINT = 'http://127.0.0.1:8790'

function normalizeEndpoint(raw: string): string {
  const trimmed = raw.trim().replace(/\/+$/, '')
  const url = new URL(trimmed) // 非法即抛
  if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('endpoint 必须是 http(s) URL')
  return trimmed
}

/** host 启动时由 index.ts 自动发现桌面端地址；浏览器半保持零 Node 依赖。 */
let endpoint = DEFAULT_ENDPOINT

export function getEndpoint(): string {
  return endpoint
}

/** 设置内核地址；非法 URL 抛错（路由层转 400）。 */
export function setEndpoint(raw: string): void {
  const trimmed = raw.trim()
  endpoint = trimmed === '' ? DEFAULT_ENDPOINT : normalizeEndpoint(trimmed)
}

/** 内核未启动时的引导文案（面板引导页 / 工具友好错误共用）。 */
export const KERNEL_DOWN_HINT =
  'AnyFind 搜索服务未启动。请先启动 AnyFind 桌面版（会自动启动本地搜索内核），然后回到本面板重试。'

/** 内核不可达错误：调用方据此输出友好提示而非堆栈。 */
export class KernelDownError extends Error {
  constructor(
    message: string,
    readonly causeMessage?: string,
  ) {
    super(message)
    this.name = 'KernelDownError'
  }
}

async function request<T>(path: string, init: RequestInit, timeoutMs: number): Promise<T> {
  let res: Response
  try {
    res = await fetch(`${endpoint}${path}`, { ...init, signal: AbortSignal.timeout(timeoutMs) })
  } catch (e) {
    throw new KernelDownError(`统一搜索内核不可达（${endpoint}）`, e instanceof Error ? e.message : String(e))
  }
  const body: unknown = await res.json().catch(() => null)
  if (!res.ok) {
    const msg = body !== null && typeof body === 'object' && 'error' in body ? String((body as { error: unknown }).error) : `HTTP ${res.status}`
    throw new Error(`内核返回错误：${msg}`)
  }
  return body as T
}

export async function kernelHealth(): Promise<HealthResponse> {
  return request<HealthResponse>('/api/health', { cache: 'no-store' }, 3000)
}

export interface SearchParams {
  q: string
  kinds?: readonly string[]
  limit?: number
  offset?: number
}

export async function kernelSearch(params: SearchParams): Promise<SearchResponse> {
  const url = new URLSearchParams()
  url.set('q', params.q)
  const kinds = (params.kinds ?? []).filter((k) => (KINDS as readonly string[]).includes(k))
  if (kinds.length > 0) url.set('kinds', kinds.join(','))
  if (params.limit !== undefined) url.set('limit', String(params.limit))
  if (params.offset !== undefined) url.set('offset', String(params.offset))
  return request<SearchResponse>(`/api/search?${url.toString()}`, { cache: 'no-store' }, 15000)
}

/** 跳回会话：spawn=false，只取命令串由调用方展示/复制（契约：dsh 插件用法）。 */
export async function kernelResume(input: { agent: string; session_id: string; cwd?: string | null }): Promise<ResumeResponse> {
  return request<ResumeResponse>('/api/resume', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ agent: input.agent, session_id: input.session_id, cwd: input.cwd ?? undefined, spawn: false }),
  }, 10000)
}

// ---------------------------------------------------------------------------
// 结果格式化（工具 / 斜杠命令共用）
// ---------------------------------------------------------------------------

/** 剥掉内核高亮 <mark> 标签，给模型/纯文本场景。 */
export function stripMark(text: string): string {
  return text.replace(/<\/?mark>/g, '')
}

/** 转义 HTML 后恢复 <mark>（面板 dangerouslySetInnerHTML 用，防索引内容 XSS）。 */
export function escapeWithMark(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/&lt;mark&gt;/g, '<mark>')
    .replace(/&lt;\/mark&gt;/g, '</mark>')
}

export function fmtMtime(mtime: string | null): string {
  if (mtime === null || mtime === '') return ''
  const d = new Date(mtime)
  // 无效/epoch 0（MFT 旧数据）显示为空，减少噪点
  if (Number.isNaN(d.getTime()) || d.getFullYear() < 1990) return ''
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function hitLine(h: Hit): string {
  const src = [h.agent, h.project, fmtMtime(h.mtime)].filter((x) => x !== null && x !== '').join(' · ')
  const title = h.title ?? h.path ?? '(无标题)'
  const snippet = h.snippet !== null && h.snippet !== '' ? `\n  ${stripMark(h.snippet).trim()}` : ''
  const sid = h.session_id !== null ? ` [session_id: ${h.session_id}]` : ''
  const path = h.path !== null && h.path !== title ? `\n  路径: ${h.path}` : ''
  return `- [${KIND_LABEL[h.kind] ?? h.kind}] ${title}（${src}）${sid}${path}${snippet}`
}

/** 展开分组为扁平命中列表（按契约组序，组内保序）。 */
export function flattenHits(resp: SearchResponse): Hit[] {
  const out: Hit[] = []
  for (const kind of KINDS) {
    for (const h of resp.groups[kind] ?? []) out.push(h)
  }
  for (const [kind, hits] of Object.entries(resp.groups)) {
    if (!(KINDS as readonly string[]).includes(kind)) out.push(...hits)
  }
  return out
}

/**
 * anyfind_search 工具输出：结构化摘要，每条 kind/title/agent/project/mtime/snippet/session_id。
 * 供模型直接引用与复述来源。
 */
export function formatForAgent(resp: SearchResponse): string {
  const hits = flattenHits(resp)
  if (hits.length === 0) {
    return `anyfind_search: 「${resp.q}」在全部历史（文件/会话/技能/记忆）中没有命中。可换关键词，或去掉过滤条件重试。`
  }
  const truncated = Object.entries(resp.truncated ?? {}).filter(([, v]) => v).map(([k]) => KIND_LABEL[k] ?? k)
  const lines = hits.map(hitLine)
  return [
    `anyfind_search「${resp.q}」命中 ${hits.length} 条` +
      (resp.took_ms !== undefined ? `（${resp.took_ms}ms）` : '') +
      (truncated.length > 0 ? `，${truncated.join('/')}组结果被截断（可加 limit 或收窄关键词）` : '') + '：',
    ...lines,
    '',
    '引用时请附来源（agent · 项目 · 日期）；session_id 可用于跳回原文会话。',
  ].join('\n')
}

/** /anyfind 斜杠命令输出：渲染进会话（宿主命令卡按纯文本渲染，不用 markdown 记号）。 */
export function formatForCommand(resp: SearchResponse): string {
  const hits = flattenHits(resp)
  if (hits.length === 0) return `统一搜索「${resp.q}」没有命中。`
  const sections: string[] = [`统一搜索「${resp.q}」命中 ${hits.length} 条：`, '']
  for (const kind of KINDS) {
    const group = resp.groups[kind] ?? []
    if (group.length === 0) continue
    sections.push(`【${KIND_LABEL[kind]}】（${group.length}）`)
    for (const h of group) {
      const src = [h.agent, h.project, fmtMtime(h.mtime)].filter((x) => x !== null && x !== '').join(' · ')
      sections.push(`- ${h.title ?? h.path ?? '(无标题)'} — ${src}`)
      if (h.snippet !== null && h.snippet !== '') sections.push(`  ${stripMark(h.snippet).trim()}`)
      if (h.session_id !== null) sections.push(`  跳回会话：session_id ${h.session_id}（面板「搜索」tab 可生成 resume 命令）`)
    }
    sections.push('')
  }
  return sections.join('\n').trimEnd()
}
