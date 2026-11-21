/**
 * dsh-anyfind host 半：把 filefind 统一搜索内核接进 dsh。
 * 注册面：
 *   - webServer 代理路由 /dsh-anyfind/api/{health,search,resume,config} 与
 *     /dsh-anyfind/api/file/{list,read,stat,op,drives}（文件端点状态码原样透传）
 *     （面板同源调用，避开浏览器直连 127.0.0.1:8790 的 CORS 问题）；
 *   - agent 工具 anyfind_search（ctx.tools）；
 *   - 斜杠命令 /anyfind（ctx.commands，结构子集类型，免依赖 dsh-commands）；
 *   - 宿主内 MCP server anyfind（ctx.mcpManager，stdio `anyfindd mcp`，
 *     三工具 anyfind_search/anyfind_file_read/anyfind_file_stat，真身
 *     core/anyfindd/src/mcp.rs tools_list）。
 * @module dsh-anyfind
 */

import { readFileSync, statSync } from 'node:fs'
import { readFile, stat } from 'node:fs/promises'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { extname, isAbsolute, join, resolve } from 'node:path'
import type { Context } from '@deepseek-ai/cordis'
// Type-only: pulls the Context.tools / Context.webServer / Context.mcpManager merges（宿主提供，不打进产物）。
import type {} from '@deepseek-ai/dsh-tools'
import type {} from '@deepseek-ai/dsh-host-webserver'
import type {} from '@deepseek-ai/dsh-mcp-manager'
import type { McpServerRegistration } from '@deepseek-ai/dsh-mcp-manager'
import { defineTool } from '@deepseek-ai/dsh-tools'
import {
  KernelDownError, KERNEL_DOWN_HINT, formatForAgent, formatForCommand,
  getEndpoint, kernelFileGet, kernelFileOp, kernelHealth, kernelResume, kernelSearch, setEndpoint,
} from './core.ts'
import type { RawResponse } from './core.ts'

/** 插件配置（cordis.patch.yml config 节）：内核地址 + 宿主内 MCP 注册。 */
export interface Config {
  endpoint?: string
  /** 宿主内 MCP 注册总开关（缺省开；false = 不注册 stdio server）。 */
  mcp?: boolean
  /** anyfindd 可执行路径（缺省自动发现：桌面端 sidecar → 开发仓构建产物）。 */
  mcpCommand?: string
  /** 传给 anyfindd 的参数（缺省 ["mcp", "--no-index"]——只读共享索引，写者留给桌面端）。 */
  mcpArgs?: string[]
}

export const name = 'dsh-anyfind'

const DEFAULT_ENDPOINT = 'http://127.0.0.1:8790'

/** 跟随桌面端自动分配的实际端口；仅 host 半读取，浏览器半不打包 Node 模块。 */
export function discoverEndpoint(configPath?: string): string {
  const path = configPath ?? (process.env.APPDATA ? join(process.env.APPDATA, 'anyfind', 'config.json') : '')
  if (path === '') return DEFAULT_ENDPOINT
  try {
    const cfg = JSON.parse(readFileSync(path, 'utf8')) as { kernel_url?: unknown }
    if (typeof cfg.kernel_url !== 'string' || cfg.kernel_url.trim() === '') return DEFAULT_ENDPOINT
    const endpoint = cfg.kernel_url.trim().replace(/\/+$/, '')
    const url = new URL(endpoint)
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return DEFAULT_ENDPOINT
    return endpoint
  } catch {
    return DEFAULT_ENDPOINT
  }
}

// ---------------------------------------------------------------------------
// /anyfind 斜杠命令：ctx.commands 结构子集（对齐 @deepseek-ai/dsh-commands 的
// CommandDefinition/CommandInvocation/CommandResult 形状；运行时是宿主同一注册表）。
// ---------------------------------------------------------------------------

interface CommandInvocationLike {
  readonly rawInput: string
  readonly signal: AbortSignal
}

type CommandResultLike =
  | { readonly kind: 'success'; readonly text?: string }
  | { readonly kind: 'error'; readonly text: string }

interface CommandsRegistryLike {
  register(definition: {
    readonly name: string
    readonly description: string
    readonly input?: { readonly hint: string }
    readonly handler: (invocation: CommandInvocationLike) => CommandResultLike | Promise<CommandResultLike>
  }): () => void
}

/** execute 返回字符串，render 包成 text block（dsh-polymarket / dsh-hippo 先例）。 */
function renderText(_args: unknown, value: unknown): Array<{ type: 'text'; text: string }> {
  return [{ type: 'text', text: String(value) }]
}

// ---------------------------------------------------------------------------
// 共享搜索动作：工具与斜杠命令同一入口；内核不在线时返回友好错误而非抛出。
// ---------------------------------------------------------------------------

async function runSearch(query: string, kinds: readonly string[] | undefined, limit: number | undefined): Promise<{ ok: true; text: string; agentText: string } | { ok: false; text: string }> {
  const q = query.trim()
  if (q === '') return { ok: false, text: '用法：/anyfind <关键词>（关键词不能为空）' }
  try {
    const resp = await kernelSearch({ q, kinds, limit })
    return { ok: true, text: formatForCommand(resp), agentText: formatForAgent(resp) }
  } catch (e) {
    if (e instanceof KernelDownError) return { ok: false, text: `${e.message}。${KERNEL_DOWN_HINT}` }
    return { ok: false, text: e instanceof Error ? e.message : String(e) }
  }
}

// ---------------------------------------------------------------------------
// agent 工具 anyfind_search
// ---------------------------------------------------------------------------

function registerAnyfindTool(ctx: Context): void {
  ctx.effect(() => ctx.tools.register(defineTool({
    name: 'anyfind_search',
    description:
      '搜索本机统一索引的全部历史：文件、文档内容、六家 AI harness（Claude Code/Codex/dsh/opencode/Pi/Cursor）的历史会话、' +
      'agent 记忆与技能文件。当用户提到"以前做过/配过/讨论过/踩过坑"某事，或需要引用历史会话与记忆的上下文时，用此工具搜索——' +
      '不要凭模型记忆猜测。支持过滤语法 kind: agent: date: ext: project:。',
    parameters: {
      query: { type: 'string', required: true, description: '搜索关键词或过滤语法组合，如 "nginx 反代" 或 "kind:session agent:codex 构建踩坑"' },
      kinds: { type: 'string', description: '逗号分隔限定类型：file,content,session,skill,memory（缺省全部）' },
      limit: { type: 'integer', description: '每类返回上限（默认 8，最大 20）' },
    },
    output: { schema: { type: 'string' }, render: renderText },
    async execute(args: { query?: string; kinds?: string; limit?: number }) {
      const query = (args.query ?? '').trim()
      if (query === '') return 'anyfind_search: query 不能为空'
      const kinds = typeof args.kinds === 'string'
        ? args.kinds.split(',').map((k) => k.trim()).filter((k) => k !== '')
        : undefined
      const limit = Math.min(Math.max(args.limit ?? 8, 1), 20)
      const r = await runSearch(query, kinds, limit)
      return r.ok ? r.agentText : r.text
    },
  })), 'dsh-anyfind: anyfind_search')
}

// ---------------------------------------------------------------------------
// 宿主内 MCP 注册：dsh 宿主的 mcpManager 服务（@deepseek-ai/dsh-mcp-manager）。
// 注册的不是三把工具本身，而是一个 stdio MCP server entry——宿主经
// mcp-manager/mcp-client 拉起 `anyfindd mcp` 子进程，tools/list 由该进程的
// core/anyfindd/src/mcp.rs 提供下面三把工具（真身 mcp.rs:218-264 tools_list）。
// ---------------------------------------------------------------------------

/** mcpManager.register 的 server 名（同时是宿主工具 namespace）。 */
export const MCP_SERVER_NAME = 'anyfind'
/** `anyfindd mcp` stdio server 暴露的工具（镜像声明，对齐 mcp.rs tools_list；宿主侧以 namespace 前缀挂载）。 */
export const MCP_TOOL_NAMES = ['anyfind_search', 'anyfind_file_read', 'anyfind_file_stat'] as const

/** 缺省注册 args：`mcp --no-index` —— stdio 子进程只读共享索引（WAL 多进程
 * 读安全，mcp.rs:467-487），写者留给桌面端 anyfindd；需要 dsh 独立建库时
 * 用 config.mcpArgs 去掉 --no-index。 */
const DEFAULT_MCP_ARGS: readonly string[] = ['mcp', '--no-index']

/** anyfindd.exe 候选位置（按序探测）：桌面端 sidecar（Tauri NSIS 装在
 * %LOCALAPPDATA%/Programs/AnyFind/）→ 开发仓 release 构建产物（桌面端
 * dev 模式同款回退，apps/desktop/src-tauri/src/lib.rs:143-152）。 */
function anyfinddCandidates(): string[] {
  const out: string[] = []
  if (process.env.LOCALAPPDATA !== undefined && process.env.LOCALAPPDATA !== '') {
    out.push(join(process.env.LOCALAPPDATA, 'Programs', 'AnyFind', 'anyfindd.exe'))
  }
  out.push('D:/coding/anyfind/core/target/release/anyfindd.exe')
  return out
}

/**
 * 发现 anyfindd 可执行路径：显式 command 优先原样采用（不探测存在，路径
 * 错误交由宿主 spawn/连接失败报错）；否则按序取第一个真实存在的候选文件；
 * 全缺 → ''（调用方跳过注册并告警）。纯函数 + 可注入候选，selftest 直接断言。
 */
export function discoverAnyfindd(command?: string, candidates?: readonly string[]): string {
  if (typeof command === 'string' && command.trim() !== '') return command.trim()
  for (const c of candidates ?? anyfinddCandidates()) {
    try {
      if (statSync(c).isFile()) return c
    } catch { /* 候选不存在，继续探测下一个 */ }
  }
  return ''
}

/**
 * 注册宿主内 MCP server。mcpManager 服务缺席（宿主未装 mcp-manager 插件）
 * 时 ctx.inject 不触发本函数，插件其余功能不受影响（与 tools/commands/
 * webServer 同一「服务缺席优雅跳过」策略）。
 */
function registerMcpServer(scope: Context, config: Config | undefined): void {
  const command = discoverAnyfindd(config?.mcpCommand)
  if (command === '') {
    scope.logger.warn('dsh-anyfind: 未找到 anyfindd 可执行文件，跳过宿主内 MCP 注册（config.mcpCommand 可显式指定路径）')
    return
  }
  const entry: McpServerRegistration = {
    transport: 'stdio',
    command,
    args: [...(config?.mcpArgs ?? DEFAULT_MCP_ARGS)],
  }
  // register(name, entry)：名字即 namespace，注册即 enabled，返回幂等 disposer；
  // registration 随本插件生命周期存续（卸载自动撤回工具），无需手动 dispose。
  scope.mcpManager.register(MCP_SERVER_NAME, entry)
  scope.logger.info(
    `dsh-anyfind: 宿主内 MCP server "${MCP_SERVER_NAME}" 已注册（${command} ${entry.args?.join(' ') ?? ''}；工具 ${MCP_TOOL_NAMES.join(' / ')}）`,
  )
}

// ---------------------------------------------------------------------------
// webServer 代理路由
// ---------------------------------------------------------------------------

function sendJson(response: ServerResponse, code: number, body: unknown): void {
  response.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' })
  response.end(JSON.stringify(body))
}

/** 同源守卫（dsh-hippo/dshmarket 先例）：带 Origin 的写请求必须与 Host 一致。 */
function sameOrigin(request: { headers: { origin?: string; host?: string } }): boolean {
  const { origin, host } = request.headers
  if (origin === undefined || host === undefined) return false
  try {
    return new URL(origin).host === host
  } catch {
    return false
  }
}

interface ConnectionAuth {
  admit(request: { headers: IncomingMessage['headers'] }): { peer: unknown } | { rejection: 401 | 403 }
}

/** 鉴权门（dsh-hippo 同型）：有 connection 服务走官方 admit，否则读放行/写要求同源。 */
function admitRequest(host: Context, request: IncomingMessage, response: ServerResponse): boolean {
  const conn = host.get('connection') as ConnectionAuth | undefined
  if (conn !== undefined && typeof conn.admit === 'function') {
    const admission = conn.admit(request)
    if ('rejection' in admission) {
      sendJson(response, admission.rejection, { error: admission.rejection === 401 ? '未认证的浏览器会话' : '请求来源不受信任' })
      return false
    }
    return true
  }
  if (request.method === 'GET' || request.method === 'HEAD') return true
  if (!sameOrigin(request)) {
    sendJson(response, 403, { error: '仅接受同源请求' })
    return false
  }
  return true
}

/** 读原始请求体（上限保护）：file/op 转发用——不解析不改写，原样交给内核判 4xx。 */
function readRawBody(request: IncomingMessage, maxBytes: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    let size = 0
    request.on('data', (chunk: Buffer) => {
      size += chunk.length
      if (size > maxBytes) {
        reject(new Error('body too large'))
        request.destroy()
        return
      }
      chunks.push(chunk)
    })
    request.on('end', () => { resolve(Buffer.concat(chunks).toString('utf8')) })
    request.on('error', reject)
  })
}

function readJsonBody(request: IncomingMessage): Promise<Record<string, unknown>> {
  return readRawBody(request, 4096).then((raw) => {
    const text = raw.trim()
    if (text === '') return {}
    try {
      return JSON.parse(text) as Record<string, unknown>
    } catch {
      throw new Error('invalid JSON body')
    }
  })
}

/** 代理调内核：KernelDownError → 502 + down 标记（面板据此渲染引导页）。 */
function proxyCall(response: ServerResponse, fn: () => Promise<unknown>): void {
  void fn().then(
    (data) => { sendJson(response, 200, data) },
    (e: unknown) => {
      if (e instanceof KernelDownError) {
        sendJson(response, 502, { ok: false, down: true, error: e.message, hint: KERNEL_DOWN_HINT, endpoint: getEndpoint() })
        return
      }
      sendJson(response, 500, { ok: false, error: e instanceof Error ? e.message : String(e) })
    },
  )
}

/**
 * 文件端点透传代理：状态码 + 响应体原样带回（内核 400/404/413/501 不改写，
 * 参数校验与钳制以内核为准，见 docs/api-contract.md v0.2）；内核 down 仍走
 * 502 + down 标记降级，与 proxyCall 同一引导语义。
 */
function proxyRaw(response: ServerResponse, fn: () => Promise<RawResponse>): void {
  void fn().then(
    ({ status, body }) => { sendJson(response, status, body) },
    (e: unknown) => {
      if (e instanceof KernelDownError) {
        sendJson(response, 502, { ok: false, down: true, error: e.message, hint: KERNEL_DOWN_HINT, endpoint: getEndpoint() })
        return
      }
      sendJson(response, 500, { ok: false, error: e instanceof Error ? e.message : String(e) })
    },
  )
}

// ---------------------------------------------------------------------------
// 面板预览端点（非内核契约，插件自有）：二进制字节 + office→PDF 转换
// ---------------------------------------------------------------------------

/** 预览字节上限（与 vendor 预览 3D 软上限 64MiB 对齐）。 */
const PREVIEW_MAX_BYTES = 64 * 1024 * 1024
/** office→PDF 输入上限（对齐 dsh office-kit maxInputBytes 默认 52428800）。 */
const OFFICE_MAX_INPUT_BYTES = 52428800
/** officeKit/officeToPdf 接受的扩展名（OfficeExtension）。 */
const OFFICE_EXTS: ReadonlySet<string> = new Set(['doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx'])

/** 结构子集：@deepseek-ai/dsh-office-kit 的 ctx.officeKit.convert（避免引入运行时依赖）。 */
interface OfficeKitSourceLike {
  key: string
  version: string
  bytes?: number
  read(signal: AbortSignal, maxBytes: number): Promise<{ bytes: Uint8Array; version: string }>
}
interface OfficeKitServiceLike {
  convert(request: { extension: string; priority: 'foreground'; source: OfficeKitSourceLike }): Promise<{ pdf: Uint8Array }>
}

/** 文件系统错误码（ENOENT/EACCES…）取值。 */
function errnoOf(e: unknown): string {
  return typeof e === 'object' && e !== null && 'code' in e ? String((e as { code: unknown }).code) : ''
}

/** 非 JSON 二进制响应（bytes/office 转 PDF 用）。 */
function sendBinary(response: ServerResponse, contentType: string, buf: Buffer): void {
  response.writeHead(200, {
    'content-type': contentType,
    'content-length': String(buf.byteLength),
    'cache-control': 'no-store',
  })
  response.end(buf)
}

function registerRoutes(ctx: Context, host: Context): Array<() => void> {
  return [
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/health',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        proxyCall(response, async () => ({ ...(await kernelHealth()), endpoint: getEndpoint() }))
      },
    }),
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/search',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        const url = new URL(request.url ?? '/', 'http://localhost')
        const q = url.searchParams.get('q') ?? ''
        const kinds = (url.searchParams.get('kinds') ?? '').split(',').map((k) => k.trim()).filter((k) => k !== '')
        const limit = Number(url.searchParams.get('limit') ?? '0') || undefined
        const offset = Number(url.searchParams.get('offset') ?? '0') || undefined
        if (q.trim() === '') {
          sendJson(response, 400, { ok: false, error: 'q 不能为空' })
          return
        }
        proxyCall(response, () => kernelSearch({ q, kinds, limit, offset }))
      },
    }),
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/resume',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        if (request.method !== 'POST') {
          response.writeHead(405, { allow: 'POST' })
          response.end()
          return
        }
        void readJsonBody(request).then(
          (body) => {
            const agent = typeof body.agent === 'string' ? body.agent : ''
            const sessionId = typeof body.session_id === 'string' ? body.session_id : ''
            if (agent === '' || sessionId === '') {
              sendJson(response, 400, { ok: false, error: 'agent 与 session_id 必填' })
              return
            }
            proxyCall(response, () => kernelResume({ agent, session_id: sessionId, cwd: typeof body.cwd === 'string' ? body.cwd : null }))
          },
          (e: unknown) => { sendJson(response, 400, { ok: false, error: e instanceof Error ? e.message : String(e) }) },
        )
      },
    }),
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/config',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        if (request.method === 'GET') {
          sendJson(response, 200, { endpoint: getEndpoint() })
          return
        }
        if (request.method !== 'POST') {
          response.writeHead(405, { allow: 'GET, POST' })
          response.end()
          return
        }
        void readJsonBody(request).then(
          (body) => {
            try {
              setEndpoint(String(body.endpoint ?? ''))
              sendJson(response, 200, { endpoint: getEndpoint() })
            } catch (e) {
              sendJson(response, 400, { ok: false, error: e instanceof Error ? e.message : String(e) })
            }
          },
          (e: unknown) => { sendJson(response, 400, { ok: false, error: e instanceof Error ? e.message : String(e) }) },
        )
      },
    }),
    // 文件端点（契约 v0.2）：query 原样转发，参数与响应结构同内核
    //（drives 无参数，query 为空串照拼）。
    ...(['list', 'read', 'stat', 'drives'] as const).map((name) => host.webServer.register({
      kind: 'exact',
      path: `/dsh-anyfind/api/file/${name}`,
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        const rawUrl = request.url ?? '/'
        const query = rawUrl.includes('?') ? rawUrl.slice(rawUrl.indexOf('?')) : ''
        proxyRaw(response, () => kernelFileGet(`/api/file/${name}`, query))
      },
    })),
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/file/op',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        if (request.method !== 'POST') {
          response.writeHead(405, { allow: 'POST' })
          response.end()
          return
        }
        // 原始体转发（上限 256KB ≫ 契约 128 条 paths 的实际体积）；非法 JSON 由内核回 400。
        readRawBody(request, 256 * 1024).then(
          (raw) => { proxyRaw(response, () => kernelFileOp(raw)) },
          (e: unknown) => { sendJson(response, 400, { error: e instanceof Error ? e.message : String(e) }) },
        )
      },
    }),
    // 预览字节端点（面板专用）：3D/压缩包/字体/图片等二进制预览取原始字节，
    // 不走内核契约（file/read 对含 NUL 文件回 binary:true 无 content）。
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/file/bytes',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        const url = new URL(request.url ?? '/', 'http://localhost')
        const target = url.searchParams.get('path') ?? ''
        if (target === '' || !isAbsolute(target)) {
          sendJson(response, 400, { error: 'path 必须是绝对路径' })
          return
        }
        void (async () => {
          const info = await stat(target)
          if (!info.isFile()) {
            sendJson(response, 400, { error: '不是普通文件' })
            return
          }
          if (info.size > PREVIEW_MAX_BYTES) {
            sendJson(response, 413, { error: `文件超过预览上限 ${PREVIEW_MAX_BYTES} 字节` })
            return
          }
          sendBinary(response, 'application/octet-stream', await readFile(target))
        })().catch((e: unknown) => {
          const code = errnoOf(e)
          if (code === 'ENOENT') { sendJson(response, 404, { error: '文件不存在' }); return }
          if (code === 'EACCES' || code === 'EPERM') { sendJson(response, 403, { error: '没有读取权限' }); return }
          sendJson(response, 500, { error: e instanceof Error ? e.message : String(e) })
        })
      },
    }),
    // office→PDF（dsh officeToPdf/officeKit 转换预览）：能力缺席 → 503，
    // 面板降级「用系统应用打开（复制路径）」。
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-anyfind/api/office/pdf',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        const url = new URL(request.url ?? '/', 'http://localhost')
        const target = url.searchParams.get('path') ?? ''
        if (target === '' || !isAbsolute(target)) {
          sendJson(response, 400, { error: 'path 必须是绝对路径' })
          return
        }
        const ext = extname(target).slice(1).toLowerCase()
        if (!OFFICE_EXTS.has(ext)) {
          sendJson(response, 400, { error: `不支持的 office 类型 .${ext}（支持 ${[...OFFICE_EXTS].join('/')}）` })
          return
        }
        const kit = host.get('officeKit') as OfficeKitServiceLike | undefined
        if (kit === undefined || typeof kit.convert !== 'function') {
          sendJson(response, 503, { error: 'officeToPdf 能力不可用（dsh officeKit 未挂载）', code: 'unavailable' })
          return
        }
        void (async () => {
          const info = await stat(target)
          if (!info.isFile()) {
            sendJson(response, 400, { error: '不是普通文件' })
            return
          }
          if (info.size > OFFICE_MAX_INPUT_BYTES) {
            sendJson(response, 413, { error: `Office 文件超过转换上限 ${OFFICE_MAX_INPUT_BYTES} 字节` })
            return
          }
          const abs = resolve(target)
          const version = `${info.mtimeMs}:${info.size}`
          const source: OfficeKitSourceLike = {
            // OfficeSourceKey 品牌在运行时是恒等函数（dsh-brand brandString），此处直接编码。
            key: JSON.stringify(['dsh-anyfind', abs]),
            version,
            bytes: info.size,
            async read(signal: AbortSignal, maxBytes: number) {
              const cur = await stat(abs)
              if (cur.size !== info.size || cur.mtimeMs !== info.mtimeMs) {
                throw Object.assign(new Error('源文件在转换期间发生变化'), { code: 'source-changed' })
              }
              if (cur.size > maxBytes) {
                throw Object.assign(new Error('源文件超过转换预留容量'), { code: 'input-too-large' })
              }
              const bytes = await readFile(abs)
              return { bytes: new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength), version }
            },
          }
          const result = await kit.convert({ extension: ext, priority: 'foreground', source })
          sendBinary(response, 'application/pdf', Buffer.from(result.pdf))
        })().catch((e: unknown) => {
          const code = errnoOf(e)
          const msg = e instanceof Error ? e.message : String(e)
          if (code === 'ENOENT') { sendJson(response, 404, { error: '文件不存在' }); return }
          if (code === 'EACCES' || code === 'EPERM') { sendJson(response, 403, { error: '没有读取权限' }); return }
          if (code === 'unavailable' || code === 'busy') { sendJson(response, 503, { error: msg, code }); return }
          if (code === 'input-too-large' || code === 'output-too-large') { sendJson(response, 413, { error: msg, code }); return }
          if (['invalid-document', 'unsupported-format', 'invalid-output', 'invalid-request', 'source-changed'].includes(code)) {
            sendJson(response, 422, { error: msg, code })
            return
          }
          sendJson(response, 500, { error: msg })
        })
      },
    }),
  ]
}

export function apply(ctx: Context, config?: Config): void {
  if (typeof config?.endpoint === 'string' && config.endpoint.trim() !== '') {
    try {
      setEndpoint(config.endpoint)
    } catch (e) {
      ctx.logger.warn(`dsh-anyfind: config.endpoint 非法（${e instanceof Error ? e.message : String(e)}），回退自动发现`)
      setEndpoint(discoverEndpoint())
    }
  } else {
    setEndpoint(discoverEndpoint())
  }
  ctx.logger.info(`dsh-anyfind: 统一搜索已加载（内核 ${getEndpoint()}）`)

  ctx.inject(['tools'], (tc) => {
    registerAnyfindTool(tc)
  })

  // 宿主内 MCP server（mcpManager 服务缺席时静默跳过；config.mcp=false 可关）。
  if (config?.mcp !== false) {
    ctx.inject(['mcpManager'], (mc) => {
      registerMcpServer(mc, config)
    })
  }

  // 斜杠命令 /anyfind：commands 服务缺席（无交互 UI 的组合）时静默跳过。
  ctx.inject(['commands'], (cc) => {
    const commands = (cc as unknown as { commands?: CommandsRegistryLike }).commands
    if (commands === undefined || typeof commands.register !== 'function') return
    cc.effect(() => commands.register({
      name: 'anyfind',
      description: '统一搜索全部历史（文件/会话/技能/记忆）',
      input: { hint: '关键词，支持 kind: agent: date: ext: project: 过滤' },
      handler: async (invocation) => {
        const r = await runSearch(invocation.rawInput, undefined, 8)
        return r.ok ? { kind: 'success' as const, text: r.text } : { kind: 'error' as const, text: r.text }
      },
    }), 'dsh-anyfind: /anyfind')
  })

  ctx.inject(['webServer'], (host) => {
    host.effect(() => {
      const disposers = registerRoutes(ctx, host)
      return () => { for (const d of disposers) d() }
    }, 'dsh-anyfind: http routes')
  })
}

// 自测/调试面：共享搜索动作从此出口透出（宿主运行时不依赖）。
export { runSearch }
// 宿主内 MCP 注册面：discoverAnyfindd / MCP_SERVER_NAME / MCP_TOOL_NAMES 已在
// 定义处直接 export（selftest 从 lib/index.js 直接断言），不再重复导出。
// 引用格式化 / 目录树文本（纯函数）：selftest 从 lib/index.js 直接断言。
export { buildTreeText, formatReference, TREE_MAX_LINES } from './core.ts'
export type { PreviewTreeNode } from './core.ts'
// file/op 请求校验（纯函数）：批量上限等断言从 lib/index.js 取。
export { FILE_OPS, FILE_OP_MAX_PATHS, validateFileOpRequest } from './core.ts'
export type { FileOp, FileOpRequest } from './core.ts'
// 收藏夹纯函数（浏览器半 localStorage store 的载荷约定）：selftest 断言增删/去重/排序。
export {
  DRIVE_TYPE_LABEL, FAVORITES_KEY, FAVORITES_MAX, favoritesAdd, favoritesHas, favoritesMove, favoritesRemove,
  favoritesToggle, normalizeFavorites,
} from './core.ts'
export type { FavoriteItem } from './core.ts'
// 压缩包条目格式化 + 划词引用格式化（纯函数）：client 浏览器半与 selftest 共用。
export {
  formatArchiveEntry, formatArchiveEntryReference, formatLineReference, formatPageReference, formatSize,
  selectionToLines,
} from './core.ts'
export type { ArchiveEntryLike } from './core.ts'
