// selftest.mjs — dsh-recall 全链路自测（不依赖真实 dsh 宿主）：
//   启动 mock 内核（test/mock-core.mjs，契约假数据）→ 以假 cordis ctx 加载
//   lib/index.js（捕获 tools/commands/webServer 注册面）→ 逐个打：
//   recall_search 工具、/recall 斜杠命令、/dsh-recall/api/* 代理路由、
//   内核 down 的友好降级。
// 用法：npm run build && npm run selftest
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const MOCK_PORT = 18765
const MOCK_ENDPOINT = `http://127.0.0.1:${MOCK_PORT}`

let passed = 0
let failed = 0
function check(name, cond, detail = '') {
  if (cond) { passed++; console.log(`  ✓ ${name}`) }
  else { failed++; console.log(`  ✗ ${name}${detail !== '' ? ` — ${detail}` : ''}`) }
}

console.log('▶ 桌面端内核地址自动发现：')
const { discoverEndpoint } = await import('../lib/index.js')
const discoveryDir = mkdtempSync(join(tmpdir(), 'anyfind-discovery-'))
const discoveryConfig = join(discoveryDir, 'config.json')
writeFileSync(discoveryConfig, JSON.stringify({ kernel_url: 'http://127.0.0.1:19321/' }))
check('读取桌面端自动分配的端口', discoverEndpoint(discoveryConfig) === 'http://127.0.0.1:19321')
writeFileSync(discoveryConfig, '{broken')
check('配置损坏时安全回退默认地址', discoverEndpoint(discoveryConfig) === 'http://127.0.0.1:8790')
rmSync(discoveryDir, { recursive: true, force: true })

// ---------------------------------------------------------------------------
// 假 cordis ctx：捕获注册面
// ---------------------------------------------------------------------------

const registered = { tools: new Map(), commands: new Map(), routes: new Map() }

function makeScope(services) {
  return {
    ...services,
    logger: { info() {}, warn() {} },
    effect(fn) { const d = fn(); return typeof d === 'function' ? d : () => {} },
    get() { return undefined },
    inject(names, cb) { return cb(makeScope(pick(names))) },
  }
}

function pick(names) {
  const services = {}
  for (const n of names) {
    if (n === 'tools') services.tools = { register(def) { registered.tools.set(def.name, def); return () => {} } }
    if (n === 'commands') services.commands = { register(def) { registered.commands.set(def.name, def); return () => {} } }
    if (n === 'webServer') services.webServer = {
      register(route) { registered.routes.set(route.path, route); return () => {} },
    }
  }
  return services
}

const rootCtx = {
  logger: { info() {}, warn() {} },
  effect(fn) { const d = fn(); return typeof d === 'function' ? d : () => {} },
  get() { return undefined },
  inject(names, cb) { return cb(makeScope(pick(names))) },
}

// ---------------------------------------------------------------------------
// 假 HTTP 请求/响应（打代理路由用）
// ---------------------------------------------------------------------------

function fakeReq({ method = 'GET', url = '/', body = null, headers = {} } = {}) {
  const listeners = {}
  return {
    method,
    url,
    headers: { host: 'localhost:3000', origin: 'http://localhost:3000', ...headers },
    on(ev, fn) { listeners[ev] = fn },
    destroy() {},
    fire() {
      if (body !== null && listeners.data) listeners.data(Buffer.from(body))
      if (listeners.end) listeners.end()
    },
  }
}

function fakeRes() {
  return {
    statusCode: 0,
    headers: {},
    body: '',
    writeHead(code, headers = {}) { this.statusCode = code; this.headers = headers },
    end(chunk = '') { this.body = String(chunk) },
    json() { return JSON.parse(this.body) },
  }
}

async function callRoute(path, { method = 'GET', query = '', body = null } = {}) {
  const route = registered.routes.get(path)
  if (route === undefined) throw new Error(`route ${path} 未注册`)
  const req = fakeReq({ method, url: query !== '' ? `${path}?${query}` : path, body })
  const res = fakeRes()
  route.handler(req, res)
  req.fire()
  // 异步 handler：轮询等响应写完
  for (let i = 0; i < 100 && res.statusCode === 0; i++) await delay(20)
  return res
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

console.log('▶ 启动 mock 内核…')
const mock = spawn(process.execPath, [join(ROOT, 'test', 'mock-core.mjs'), String(MOCK_PORT)], { stdio: 'pipe' })
await new Promise((resolveReady, rejectReady) => {
  mock.stdout.on('data', (d) => { if (String(d).includes('listening')) resolveReady() })
  mock.on('error', rejectReady)
  mock.on('exit', (code) => { rejectReady(new Error(`mock 过早退出: ${code}`)) })
  setTimeout(() => { rejectReady(new Error('mock 启动超时')) }, 5000)
})

try {
  console.log('▶ 加载插件（假 cordis ctx，config.endpoint 指向 mock）…')
  const { apply } = await import('../lib/index.js')
  apply(rootCtx, { endpoint: MOCK_ENDPOINT })

  console.log('▶ 注册面：')
  check('recall_search 工具已注册', registered.tools.has('recall_search'))
  check('/recall 斜杠命令已注册', registered.commands.has('recall'))
  for (const p of ['/dsh-recall/api/health', '/dsh-recall/api/search', '/dsh-recall/api/resume', '/dsh-recall/api/config']) {
    check(`路由 ${p} 已注册`, registered.routes.has(p))
  }
  const tool = registered.tools.get('recall_search')
  check('工具描述含使用时机指引', typeof tool.description === 'string' && tool.description.includes('以前'))

  console.log('▶ recall_search 工具（内核在线）：')
  const toolOut = await tool.execute({ query: 'nginx' }, {})
  check('返回命中摘要', typeof toolOut === 'string' && toolOut.includes('命中'), String(toolOut).slice(0, 80))
  check('摘要含结构化字段（agent/项目/session_id）',
    toolOut.includes('claude-code') && toolOut.includes('session_id') && toolOut.includes('sess-abc-123'))
  check('摘要无 <mark> 残留（模型面纯文本）', !toolOut.includes('<mark>'))
  const toolFiltered = await tool.execute({ query: 'nginx', kinds: 'session', limit: 5 }, {})
  check('kinds 过滤生效（仅会话组）', toolFiltered.includes('会话') && !toolFiltered.includes('[文件]'))
  const toolEmpty = await tool.execute({ query: '' }, {})
  check('空 query 友好报错', toolEmpty.includes('query 不能为空'))

  console.log('▶ /recall 斜杠命令：')
  const cmd = registered.commands.get('recall')
  const cmdOk = await cmd.handler({ rawInput: ' nginx', signal: new AbortController().signal })
  check('成功结果 kind=success', cmdOk.kind === 'success')
  check('渲染含来源标注（agent · 项目 · 日期）',
    cmdOk.text.includes('claude-code') && cmdOk.text.includes('2026-10-18'), cmdOk.text.slice(0, 120))
  check('渲染含分组标题', cmdOk.text.includes('【会话】') && cmdOk.text.includes('【记忆】'))
  const cmdUsage = await cmd.handler({ rawInput: '  ', signal: new AbortController().signal })
  check('空关键词返回用法错误', cmdUsage.kind === 'error' && cmdUsage.text.includes('用法'))
  const cmdNone = await cmd.handler({ rawInput: ' zz-nothing', signal: new AbortController().signal })
  check('无命中正常返回（mock 全组返回也接受）', cmdNone.kind === 'success')

  console.log('▶ 代理路由：')
  const health = await callRoute('/dsh-recall/api/health')
  check('GET health 200 且透传索引计数', health.statusCode === 200 && health.json().index.sessions === 86)
  const search = await callRoute('/dsh-recall/api/search', { query: 'q=nginx&kinds=session' })
  check('GET search 透传 groups', search.statusCode === 200 && search.json().groups.session.length === 2)
  const searchEmpty = await callRoute('/dsh-recall/api/search', { query: 'q=' })
  check('GET search 空 q → 400', searchEmpty.statusCode === 400)
  const resume = await callRoute('/dsh-recall/api/resume', {
    method: 'POST',
    body: JSON.stringify({ agent: 'claude-code', session_id: 'sess-abc-123', cwd: 'D:/coding/filefind/dsh-plugin' }),
    headers: {},
  })
  check('POST resume 返回命令串', resume.statusCode === 200 && resume.json().command === 'claude --resume sess-abc-123', resume.body)
  const cfgGet = await callRoute('/dsh-recall/api/config')
  check('GET config 返回当前 endpoint', cfgGet.statusCode === 200 && cfgGet.json().endpoint === MOCK_ENDPOINT)
  const cfgBad = await callRoute('/dsh-recall/api/config', { method: 'POST', body: JSON.stringify({ endpoint: 'not a url' }) })
  check('POST config 非法地址 → 400', cfgBad.statusCode === 400)

  console.log('▶ 内核 down 降级：')
  mock.kill()
  await delay(300)
  const healthDown = await callRoute('/dsh-recall/api/health')
  check('health → 502 + down 标记 + 引导', healthDown.statusCode === 502 && healthDown.json().down === true && healthDown.json().hint.includes('AnyFind 桌面版'))
  const toolDown = await tool.execute({ query: 'nginx' }, {})
  check('工具返回友好错误而非抛出', typeof toolDown === 'string' && toolDown.includes('内核不可达') && toolDown.includes('未启动'))
  const cmdDown = await cmd.handler({ rawInput: ' nginx', signal: new AbortController().signal })
  check('斜杠命令返回友好错误', cmdDown.kind === 'error' && cmdDown.text.includes('未启动'))
} finally {
  mock.kill()
}

console.log(`\n${failed === 0 ? '✅' : '❌'} selftest: ${passed} passed, ${failed} failed`)
process.exit(failed === 0 ? 0 : 1)
