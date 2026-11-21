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
  is_dir: boolean | null
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
// 文件端点（契约 v0.2，docs/api-contract.md「文件端点」组）：
// 代理层只做状态码 + 响应体的原样透传，参数语义（钳制/校验/404/413/501）
// 完全由内核解析——这里不复刻内核规则，避免两边判断漂移。
// ---------------------------------------------------------------------------

/** 契约：GET /api/file/list 200 条目（mtime 为本地时区 ISO 串，取不到 null）。 */
export interface FileEntry {
  name: string
  is_dir: boolean
  size: number
  mtime: string | null
}

/** 契约：GET /api/file/list 200 响应。 */
export interface FileListResponse {
  path: string
  entries: FileEntry[]
  truncated: boolean
}

/** 契约：GET /api/file/stat 200 响应。 */
export interface FileStatResponse {
  name: string
  is_dir: boolean
  size: number
  mtime: string | null
}

/** 契约：GET /api/file/read 200 响应（binary:true 时无 content 等字段）。 */
export interface FileReadResponse {
  binary: boolean
  content?: string
  totalLines?: number
  offset?: number
  lines?: number
  truncated?: boolean
  /** 契约 v0.2 补丁：format=base64 时返回 {content_base64, binary, truncated}（≤2MB 整文件；文本与二进制通用），内核不回 size。 */
  content_base64?: string
}

/** 契约：POST /api/file/op 200 单项结果（项级失败仍是 200 + ok:false）。 */
export interface FileOpResult {
  path: string
  ok: boolean
  error?: string
}

/** 契约：POST /api/file/op 200 响应。 */
export interface FileOpResponse {
  ok: boolean
  results: FileOpResult[]
}

/** 契约：GET /api/file/drives 200 条目（Windows 盘符；非 Windows 固定 "/"）。 */
export interface DriveEntry {
  path: string
  type: 'fixed' | 'removable' | 'cdrom' | 'ram' | 'unknown'
}

/** 契约：GET /api/file/drives 200 响应。 */
export interface FileDrivesResponse {
  drives: DriveEntry[]
}

/** 内核原始回包：状态码与体原样带回（400/404/413/501 不改写）。 */
export interface RawResponse {
  status: number
  body: unknown
}

/** 只在网络层失败时抛 KernelDownError；HTTP 错误状态按原样返回。 */
async function rawRequest(path: string, init: RequestInit, timeoutMs: number): Promise<RawResponse> {
  let res: Response
  try {
    res = await fetch(`${endpoint}${path}`, { ...init, signal: AbortSignal.timeout(timeoutMs) })
  } catch (e) {
    throw new KernelDownError(`统一搜索内核不可达（${endpoint}）`, e instanceof Error ? e.message : String(e))
  }
  const body: unknown = await res.json().catch(() => null)
  if (body === null) return { status: res.status, body: { error: `HTTP ${res.status}` } }
  return { status: res.status, body }
}

const FILE_GET_PATHS = new Set(['/api/file/list', '/api/file/read', '/api/file/stat', '/api/file/drives'])

/**
 * 文件 GET 端点透传：`query` 是浏览器原始 query 串（含 `?`），
 * 原样拼给内核——max/noise/offset/lines 的钳制与越界语义以内核为准。
 */
export async function kernelFileGet(subpath: string, query: string, timeoutMs = 20000): Promise<RawResponse> {
  if (!FILE_GET_PATHS.has(subpath)) throw new Error(`未知文件端点：${subpath}`)
  return rawRequest(`${subpath}${query}`, { cache: 'no-store' }, timeoutMs)
}

/**
 * POST /api/file/op 透传：`body` 为浏览器原始请求体（不解析不改写），
 * 非法 JSON / op 不支持 / paths 超 128 等由内核按契约回 4xx。
 */
export async function kernelFileOp(body: string): Promise<RawResponse> {
  return rawRequest('/api/file/op', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body,
    cache: 'no-store',
  }, 60000)
}

// ---------------------------------------------------------------------------
// 引用文本 / 目录树文本（纯函数：浏览器半与 selftest 共用，零 DOM 依赖）
// ---------------------------------------------------------------------------

/**
 * 「引用」写进输入框的文本：非空路径按行拼接（单条即原样返回）。
 * 输入框侧的换行胶水（末尾无换行时补一个 \n）由 bridge 负责，这里只管内容。
 */
export function formatReference(paths: readonly string[]): string {
  return paths.map((p) => p.trim()).filter((p) => p !== '').join('\n')
}

/** 目录树节点：拖拽目录生成 ├─└─│ 文本树的输入（children 缺省=文件）。 */
export interface PreviewTreeNode {
  name: string
  children?: readonly PreviewTreeNode[]
}

/** 文本树行数上限（任务约定 ≤200 行；超出截断并附说明行）。 */
export const TREE_MAX_LINES = 200

/**
 * 目录树 → ├─└─│ 文本：
 * ```
 * D:\dir
 * ├─ a.txt
 * └─ sub
 *     ├─ b.txt
 *     └─ c.txt
 * ```
 * 超过 `maxLines`（默认 200）时保留前 maxLines-1 行 + 1 行截断说明。
 */
export function buildTreeText(nodes: readonly PreviewTreeNode[], maxLines: number = TREE_MAX_LINES): string {
  const cap = Math.max(1, Math.trunc(maxLines))
  const lines: string[] = []
  const walk = (children: readonly PreviewTreeNode[], prefix: string): void => {
    children.forEach((child, i) => {
      const last = i === children.length - 1
      lines.push(`${prefix}${last ? '└─ ' : '├─ '}${child.name}`)
      if (child.children !== undefined && child.children.length > 0) {
        walk(child.children, prefix + (last ? '   ' : '│  '))
      }
    })
  }
  for (const root of nodes) {
    lines.push(root.name)
    if (root.children !== undefined && root.children.length > 0) walk(root.children, '')
  }
  if (lines.length <= cap) return lines.join('\n')
  const head = lines.slice(0, cap - 1)
  head.push(`…（已截断，共 ${lines.length} 行）`)
  return head.join('\n')
}

// ---------------------------------------------------------------------------
// 文件写操作请求校验（纯函数）：客户端提交前预检 + selftest 断言。
// 规则与契约 v0.2 /api/file/op 的 4xx 判定同源（op 白名单 / paths 1..128 /
// copy·cut 缺 target / rename·newFile·newDir 缺 newName / 单条约束）。
// 代理层仍按「参数与响应结构同内核」原样转发，这里只做发请求前的友好提示。
// ---------------------------------------------------------------------------

/** 契约：POST /api/file/op 的 op 白名单。 */
export const FILE_OPS = ['rename', 'copy', 'cut', 'recycle', 'newFile', 'newDir'] as const

export type FileOp = (typeof FILE_OPS)[number]

/** 契约：paths 上限（超出 → 400）。 */
export const FILE_OP_MAX_PATHS = 128

/** 契约：POST /api/file/op 请求体。 */
export interface FileOpRequest {
  op: FileOp
  paths: string[]
  target?: string
  newName?: string
}

/**
 * 校验 file/op 请求体：通过返回 null，否则返回错误文案（与内核 400 {error} 同形）。
 * @param body - 待校验请求体（任意 unknown）
 */
export function validateFileOpRequest(body: unknown): string | null {
  if (body === null || typeof body !== 'object' || Array.isArray(body)) return '请求体必须是 JSON 对象'
  const b = body as Record<string, unknown>
  if (typeof b.op !== 'string' || !(FILE_OPS as readonly string[]).includes(b.op)) {
    return `op 不支持：${String(b.op)}（可选 ${FILE_OPS.join('/')}）`
  }
  const op = b.op as FileOp
  if (!Array.isArray(b.paths)) return 'paths 必须是数组'
  const paths = b.paths as unknown[]
  if (paths.length === 0) return 'paths 不能为空'
  if (paths.length > FILE_OP_MAX_PATHS) return `paths 超过上限 ${FILE_OP_MAX_PATHS}（当前 ${paths.length} 条）`
  for (const p of paths) {
    if (typeof p !== 'string' || p.trim() === '') return 'paths 必须是非空绝对路径字符串'
  }
  const target = typeof b.target === 'string' ? b.target.trim() : ''
  const newName = typeof b.newName === 'string' ? b.newName.trim() : ''
  if ((op === 'copy' || op === 'cut') && target === '') return `${op} 需要 target 目标目录`
  if ((op === 'rename' || op === 'newFile' || op === 'newDir') && newName === '') return `${op} 需要 newName`
  if ((op === 'rename' || op === 'newFile' || op === 'newDir') && paths.length !== 1) {
    return `${op} 只接受单条 path（当前 ${paths.length} 条）`
  }
  if (op === 'rename' && target === '') return 'rename 需要 target（所在目录）'
  return null
}

// ---------------------------------------------------------------------------
// 收藏夹（纯函数）：localStorage 键 anyfind.favorites 的载荷约定——
// FavoriteItem[]（JSON）。浏览器半用模块级 store 包装（持久化 + storage 事件
// 跨 tab 同步），selftest 直接断言这些纯函数（增删/去重/拖拽排序）。
// ---------------------------------------------------------------------------

/** 契约：收藏夹 localStorage 键。 */
export const FAVORITES_KEY = 'anyfind.favorites'
/** 收藏条目上限（超出丢弃最旧——列表尾部）。 */
export const FAVORITES_MAX = 200

export interface FavoriteItem {
  path: string
  name: string
  isDir: boolean
  addedAt: number
}

/** 宽容识别（v0.3.1 终审修复）：canonical {isDir,addedAt} 与桌面端旧形态
 * {is_dir, 无 addedAt} 均接受——两端同键 anyfind.favorites，载荷必须互通。 */
function isFavoriteLike(x: unknown): x is { path: string; name: string; isDir: boolean; addedAt: number | undefined } {
  if (x === null || typeof x !== 'object' || Array.isArray(x)) return false
  const it = x as Record<string, unknown>
  if (typeof it.path !== 'string' || it.path.trim() === '' || typeof it.name !== 'string') return false
  const dir = it.isDir ?? it.is_dir
  return typeof dir === 'boolean'
}

/** 解析 localStorage 载荷：过滤畸形项、按 path 去重、截到 FAVORITES_MAX；
 * 旧形态 addedAt 缺省补当前时刻（排序语义：新导入视为最新）。 */
export function normalizeFavorites(raw: unknown): FavoriteItem[] {
  const list = Array.isArray(raw) ? raw : []
  const seen = new Set<string>()
  const out: FavoriteItem[] = []
  for (const x of list) {
    if (out.length >= FAVORITES_MAX) break
    if (!isFavoriteLike(x)) continue
    const path = x.path.trim()
    if (path === '' || seen.has(path)) continue
    seen.add(path)
    // 运行时值可能在 is_dir 别名上——从原始对象强制转换，别信守卫的 narrowing。
    const raw = x as { path: string; name: string; isDir?: unknown; is_dir?: unknown; addedAt?: unknown }
    const isDir = (raw.isDir ?? raw.is_dir) === true
    out.push({ path, name: raw.name, isDir, addedAt: typeof raw.addedAt === 'number' ? raw.addedAt : Date.now() })
  }
  return out
}

export function favoritesHas(list: readonly FavoriteItem[], path: string): boolean {
  const p = path.trim()
  return p !== '' && list.some((x) => x.path === p)
}

/** 收藏（去重 + 头插 + 上限截断）：已在收藏时原样返回同一引用。 */
export function favoritesAdd(list: readonly FavoriteItem[], item: FavoriteItem): FavoriteItem[] {
  const path = item.path.trim()
  if (path === '') return list as FavoriteItem[]
  if (favoritesHas(list, path)) return list as FavoriteItem[]
  const next = [{ ...item, path }, ...list]
  return next.length > FAVORITES_MAX ? next.slice(0, FAVORITES_MAX) : next
}

export function favoritesRemove(list: readonly FavoriteItem[], path: string): FavoriteItem[] {
  const p = path.trim()
  if (p === '') return list as FavoriteItem[]
  const next = list.filter((x) => x.path !== p)
  return next.length === list.length ? list as FavoriteItem[] : next
}

/** 收藏/取消收藏切换（星标按钮共用）。 */
export function favoritesToggle(list: readonly FavoriteItem[], item: FavoriteItem): FavoriteItem[] {
  return favoritesHas(list, item.path) ? favoritesRemove(list, item.path) : favoritesAdd(list, item)
}

/** 拖拽排序：把 from 项移动到 to 位（双向 clamp，非法/原位返回同一引用）。 */
export function favoritesMove(list: readonly FavoriteItem[], from: number, to: number): FavoriteItem[] {
  if (list.length === 0) return list as FavoriteItem[]
  const f = Math.trunc(from)
  const t = Math.trunc(to)
  if (f < 0 || f >= list.length || t < 0 || t >= list.length || f === t) return list as FavoriteItem[]
  const next = [...list]
  const moved = next.splice(f, 1)[0]
  next.splice(t, 0, moved)
  return next
}

/** 盘符 type → 展示名（file/drives 条目；client 图标与 selftest 断言共用）。 */
export const DRIVE_TYPE_LABEL: Readonly<Record<string, string>> = Object.freeze({
  fixed: '本地磁盘',
  removable: '可移动磁盘',
  cdrom: '光盘驱动器',
  ram: '内存磁盘',
  unknown: '其他',
})

// ---------------------------------------------------------------------------
// 压缩包条目格式化（纯函数）：client 压缩包浏览器行展示 / selftest 断言。
// entry 形状对齐 src/vendor/archive（path/dir/size/method/encrypted）。
// ---------------------------------------------------------------------------

/** 确定性字节数展示（core 自持；client 树行另有 fmtBytes，不互相依赖）。 */
export function formatSize(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B'
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`
  return `${(n / 1024 ** 3).toFixed(2)} GB`
}

export interface ArchiveEntryLike {
  path: string
  dir: boolean
  size?: number
  method?: string
  encrypted?: boolean
}

/** 条目单行展示：`名字/  大小  方法[ ·已加密]`（目录尾 /、大小 —）。 */
export function formatArchiveEntry(entry: ArchiveEntryLike): string {
  const name = entry.dir ? `${entry.path}/` : entry.path
  const size = entry.dir ? '—' : formatSize(entry.size ?? 0)
  const method = entry.method !== undefined && entry.method !== '' ? `  ${entry.method}` : ''
  const enc = entry.encrypted === true ? '  ·已加密' : ''
  return `${name}  ${size}${method}${enc}`
}

/** 压缩包条目引用串：`D:/a.zip!/dir/b.txt`（「引用」按钮与拖拽共用同一格式）。 */
export function formatArchiveEntryReference(archivePath: string, entryPath: string): string {
  const a = archivePath.trim()
  const e = entryPath.replace(/^\/+/, '')
  if (a === '' || e === '') return ''
  return `${a}!/${e}`
}

// ---------------------------------------------------------------------------
// 划词引用（纯函数）：文本预览选中行 → `路径:起-止`、PDF 文本层选区 →
// `路径:第 N 页`，均附所选文本首行摘要；selftest 断言固定格式。
// ---------------------------------------------------------------------------

/** 摘要：所选文本第一个非空行，折叠空白，>160 字符截断加 …。 */
function firstLineSummary(selectedText: string): string {
  const lines = selectedText.split('\n')
  for (const line of lines) {
    const t = line.replace(/\s+/g, ' ').trim()
    if (t !== '') return t.length > 160 ? `${t.slice(0, 160)}…` : t
  }
  return ''
}

/**
 * 全文 + 选区字符偏移 → 行号区间与所选文本（collapsed 选区返回 null）。
 * @param text 预览窗口全文（pre 的 textContent）
 * @param start 选区起点偏移（clamp 到 [0, text.length]）
 * @param end 选区终点偏移
 * @param firstLineNumber 全文第一行的行号（分页窗口起点，1-based）
 */
export function selectionToLines(
  text: string, start: number, end: number, firstLineNumber: number,
): { start: number; end: number; text: string } | null {
  const s = Math.min(Math.max(Math.trunc(start), 0), text.length)
  const e = Math.min(Math.max(Math.trunc(end), 0), text.length)
  if (s >= e) return null
  const selected = text.slice(s, e)
  const lineAt = (off: number): number => {
    let n = 0
    for (let i = 0; i < off; i++) if (text.charCodeAt(i) === 10) n++
    return n
  }
  return {
    start: firstLineNumber + lineAt(s),
    end: firstLineNumber + lineAt(e - 1),
    text: selected,
  }
}

/** 文本划词引用：`路径:起始行-结束行` + 首行摘要行（空选区返回 ''）。 */
export function formatLineReference(path: string, startLine: number, endLine: number, selectedText: string): string {
  const p = path.trim()
  const summary = firstLineSummary(selectedText)
  if (p === '' || summary === '') return ''
  return `${p}:${startLine}-${endLine}\n> ${summary}`
}

/** PDF 文本层划词引用：`路径:第 N 页` + 首行摘要行（空选区返回 ''）。 */
export function formatPageReference(path: string, page: number, selectedText: string): string {
  const p = path.trim()
  const summary = firstLineSummary(selectedText)
  if (p === '' || summary === '') return ''
  return `${p}:第 ${page} 页\n> ${summary}`
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
