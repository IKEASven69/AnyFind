/**
 * dsh-recall host 半：把 filefind 统一搜索内核接进 dsh。
 * 注册面：
 *   - webServer 代理路由 /dsh-recall/api/{health,search,resume,config}
 *     （面板同源调用，避开浏览器直连 127.0.0.1:8790 的 CORS 问题）；
 *   - agent 工具 recall_search（ctx.tools）；
 *   - 斜杠命令 /recall（ctx.commands，结构子集类型，免依赖 dsh-commands）。
 * @module dsh-recall
 */

import type { IncomingMessage, ServerResponse } from 'node:http'
import type { Context } from '@deepseek-ai/cordis'
// Type-only: pulls the Context.tools / Context.webServer merges（宿主提供，不打进产物）。
import type {} from '@deepseek-ai/dsh-tools'
import type {} from '@deepseek-ai/dsh-host-webserver'
import { defineTool } from '@deepseek-ai/dsh-tools'
import {
  KernelDownError, KERNEL_DOWN_HINT, formatForAgent, formatForCommand,
  getEndpoint, kernelHealth, kernelResume, kernelSearch, setEndpoint,
} from './core.ts'

/** 插件配置（cordis.patch.yml config 节）：内核地址。 */
export interface Config {
  endpoint?: string
}

export const name = 'dsh-recall'

// ---------------------------------------------------------------------------
// /recall 斜杠命令：ctx.commands 结构子集（对齐 @deepseek-ai/dsh-commands 的
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
  if (q === '') return { ok: false, text: '用法：/recall <关键词>（关键词不能为空）' }
  try {
    const resp = await kernelSearch({ q, kinds, limit })
    return { ok: true, text: formatForCommand(resp), agentText: formatForAgent(resp) }
  } catch (e) {
    if (e instanceof KernelDownError) return { ok: false, text: `${e.message}。${KERNEL_DOWN_HINT}` }
    return { ok: false, text: e instanceof Error ? e.message : String(e) }
  }
}

// ---------------------------------------------------------------------------
// agent 工具 recall_search
// ---------------------------------------------------------------------------

function registerRecallTool(ctx: Context): void {
  ctx.effect(() => ctx.tools.register(defineTool({
    name: 'recall_search',
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
      if (query === '') return 'recall_search: query 不能为空'
      const kinds = typeof args.kinds === 'string'
        ? args.kinds.split(',').map((k) => k.trim()).filter((k) => k !== '')
        : undefined
      const limit = Math.min(Math.max(args.limit ?? 8, 1), 20)
      const r = await runSearch(query, kinds, limit)
      return r.ok ? r.agentText : r.text
    },
  })), 'dsh-recall: recall_search')
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

function readJsonBody(request: IncomingMessage): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    let size = 0
    request.on('data', (chunk: Buffer) => {
      size += chunk.length
      if (size > 4096) {
        reject(new Error('body too large'))
        request.destroy()
        return
      }
      chunks.push(chunk)
    })
    request.on('end', () => {
      try {
        const raw = Buffer.concat(chunks).toString('utf8').trim()
        resolve(raw === '' ? {} : JSON.parse(raw) as Record<string, unknown>)
      } catch {
        reject(new Error('invalid JSON body'))
      }
    })
    request.on('error', reject)
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

function registerRoutes(ctx: Context, host: Context): Array<() => void> {
  return [
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-recall/api/health',
      handler: (request, response) => {
        if (!admitRequest(host, request, response)) return
        proxyCall(response, async () => ({ ...(await kernelHealth()), endpoint: getEndpoint() }))
      },
    }),
    host.webServer.register({
      kind: 'exact',
      path: '/dsh-recall/api/search',
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
      path: '/dsh-recall/api/resume',
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
      path: '/dsh-recall/api/config',
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
  ]
}

export function apply(ctx: Context, config?: Config): void {
  if (typeof config?.endpoint === 'string' && config.endpoint.trim() !== '') {
    try {
      setEndpoint(config.endpoint)
    } catch (e) {
      ctx.logger.warn(`dsh-recall: config.endpoint 非法（${e instanceof Error ? e.message : String(e)}），回退默认 ${getEndpoint()}`)
    }
  }
  ctx.logger.info(`dsh-recall: 统一搜索已加载（内核 ${getEndpoint()}）`)

  ctx.inject(['tools'], (tc) => {
    registerRecallTool(tc)
  })

  // 斜杠命令 /recall：commands 服务缺席（无交互 UI 的组合）时静默跳过。
  ctx.inject(['commands'], (cc) => {
    const commands = (cc as unknown as { commands?: CommandsRegistryLike }).commands
    if (commands === undefined || typeof commands.register !== 'function') return
    cc.effect(() => commands.register({
      name: 'recall',
      description: '统一搜索全部历史（文件/会话/技能/记忆）',
      input: { hint: '关键词，支持 kind: agent: date: ext: project: 过滤' },
      handler: async (invocation) => {
        const r = await runSearch(invocation.rawInput, undefined, 8)
        return r.ok ? { kind: 'success' as const, text: r.text } : { kind: 'error' as const, text: r.text }
      },
    }), 'dsh-recall: /recall')
  })

  ctx.inject(['webServer'], (host) => {
    host.effect(() => {
      const disposers = registerRoutes(ctx, host)
      return () => { for (const d of disposers) d() }
    }, 'dsh-recall: http routes')
  })
}

// 自测/调试面：共享搜索动作从此出口透出（宿主运行时不依赖）。
export { runSearch }
