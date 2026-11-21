// selftest.mjs — dsh-anyfind 全链路自测（不依赖真实 dsh 宿主）：
//   启动 mock 内核（test/mock-core.mjs，契约假数据）→ 以假 cordis ctx 加载
//   lib/index.js（捕获 tools/commands/webServer 注册面）→ 逐个打：
//   anyfind_search 工具、/anyfind 斜杠命令、/dsh-anyfind/api/* 代理路由、
//   /dsh-anyfind/api/file/{list,read,stat,op,drives} 文件端点透传（契约 v0.2，
//   op 含成功/单 path 失败两态与 4xx；read 含 format=base64 字节通路；
//   drives 为 2026-11-21 补充的盘符枚举）、
//   /api/file/bytes 预览字节端点、
//   /api/office/pdf 转换端点（officeKit 缺席 → 503 降级）、批量上限校验纯函数、
//   lib/client.js 工厂求值（vendor 预览 react 注入）、
//   内核 down 的友好降级。
// 用法：npm run build && npm run selftest
import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { deflateRawSync } from 'node:zlib'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
// 固定端口会与本机无关应用撞车（实测：veyra_qml_ui.exe 占用 18765 导致 mock
// EADDRINUSE 退出、selftest 挂在启动段）——改绑 :0 由内核分配空闲端口。
function findFreePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer()
    srv.unref()
    srv.on('error', reject)
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address()
      srv.close(() => resolve(port))
    })
  })
}
const MOCK_PORT = await findFreePort()
const MOCK_ENDPOINT = `http://127.0.0.1:${MOCK_PORT}`

let passed = 0
let failed = 0
function check(name, cond, detail = '') {
  if (cond) { passed++; console.log(`  ✓ ${name}`) }
  else { failed++; console.log(`  ✗ ${name}${detail !== '' ? ` — ${detail}` : ''}`) }
}

console.log('▶ 桌面端内核地址自动发现：')
const {
  discoverEndpoint, discoverAnyfindd, MCP_SERVER_NAME, MCP_TOOL_NAMES, formatReference, buildTreeText,
  TREE_MAX_LINES, FILE_OP_MAX_PATHS, validateFileOpRequest,
  // 收藏夹纯函数（文件工作台六件①：localStorage 载荷约定）
  FAVORITES_KEY, FAVORITES_MAX, normalizeFavorites, favoritesAdd, favoritesRemove, favoritesMove,
  favoritesToggle, favoritesHas, DRIVE_TYPE_LABEL,
  // 压缩包条目格式化（④）+ 划词引用格式化（⑥）
  formatArchiveEntry, formatArchiveEntryReference, formatLineReference, formatPageReference, selectionToLines,
} = await import('../lib/index.js')
const discoveryDir = mkdtempSync(join(tmpdir(), 'anyfind-discovery-'))
const discoveryConfig = join(discoveryDir, 'config.json')
writeFileSync(discoveryConfig, JSON.stringify({ kernel_url: 'http://127.0.0.1:19321/' }))
check('读取桌面端自动分配的端口', discoverEndpoint(discoveryConfig) === 'http://127.0.0.1:19321')
writeFileSync(discoveryConfig, '{broken')
check('配置损坏时安全回退默认地址', discoverEndpoint(discoveryConfig) === 'http://127.0.0.1:8790')
rmSync(discoveryDir, { recursive: true, force: true })

console.log('▶ anyfindd 可执行发现（纯函数，宿主内 MCP 注册用）：')
check('显式 command 原样采用（不探测存在）', discoverAnyfindd('  C:/custom/anyfindd.exe ') === 'C:/custom/anyfindd.exe')
check('空白 command 不算显式（落回候选探测）', discoverAnyfindd('  ', ['C:/missing/anyfindd.exe']) === '')
const probeDir = mkdtempSync(join(tmpdir(), 'anyfind-mcp-probe-'))
const present = join(probeDir, 'anyfindd.exe')
writeFileSync(present, '')
check('候选按序取第一个真实文件', discoverAnyfindd(undefined, ['C:/missing/a.exe', present, 'C:/missing/b.exe']) === present)
mkdirSync(join(probeDir, 'dir-like'))
check('目录不算可执行候选', discoverAnyfindd(undefined, [join(probeDir, 'dir-like')]) === '')
check('候选全缺返回空串（调用方跳过注册）', discoverAnyfindd(undefined, ['C:/missing/a.exe', 'C:/missing/b.exe']) === '')
rmSync(probeDir, { recursive: true, force: true })
check('MCP 工具镜像声明与 mcp.rs tools_list 对齐（三工具）',
  MCP_SERVER_NAME === 'anyfind' &&
  [...MCP_TOOL_NAMES].join(',') === 'anyfind_search,anyfind_file_read,anyfind_file_stat')

// ---------------------------------------------------------------------------
// 假 cordis ctx：捕获注册面
// ---------------------------------------------------------------------------

const registered = { tools: new Map(), commands: new Map(), routes: new Map(), mcp: new Map() }

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
    if (n === 'mcpManager') services.mcpManager = {
      register(name, entry) { registered.mcp.set(name, entry); return () => { registered.mcp.delete(name) } },
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
    /** 二进制响应（file/bytes 等 sendBinary）保留原始字节；文本仍走 body 字符串。 */
    buffer: null,
    writeHead(code, headers = {}) { this.statusCode = code; this.headers = headers },
    end(chunk = '') {
      if (typeof chunk === 'string') {
        this.body = chunk
        this.buffer = Buffer.from(chunk, 'utf8')
      } else {
        this.buffer = Buffer.from(chunk)
        this.body = ''
      }
    },
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
  check('anyfind_search 工具已注册', registered.tools.has('anyfind_search'))
  check('/anyfind 斜杠命令已注册', registered.commands.has('anyfind'))
  for (const p of ['/dsh-anyfind/api/health', '/dsh-anyfind/api/search', '/dsh-anyfind/api/resume', '/dsh-anyfind/api/config']) {
    check(`路由 ${p} 已注册`, registered.routes.has(p))
  }
  for (const p of ['/dsh-anyfind/api/file/list', '/dsh-anyfind/api/file/read', '/dsh-anyfind/api/file/stat', '/dsh-anyfind/api/file/op', '/dsh-anyfind/api/file/drives']) {
    check(`文件路由 ${p} 已注册`, registered.routes.has(p))
  }
  for (const p of ['/dsh-anyfind/api/file/bytes', '/dsh-anyfind/api/office/pdf']) {
    check(`预览路由 ${p} 已注册`, registered.routes.has(p))
  }
  const tool = registered.tools.get('anyfind_search')
  check('工具描述含使用时机指引', typeof tool.description === 'string' && tool.description.includes('以前'))

  console.log('▶ 宿主内 MCP 注册（mcpManager 注册面）：')
  const mcpEntry = registered.mcp.get('anyfind')
  check('stdio server anyfind 已注册', mcpEntry !== undefined)
  check('transport=stdio 且 command 非空（发现 anyfindd）',
    mcpEntry?.transport === 'stdio' && typeof mcpEntry?.command === 'string' && mcpEntry.command !== '')
  check('command 与 discoverAnyfindd() 一致（本机候选探测）', mcpEntry?.command === discoverAnyfindd(undefined))
  check('缺省 args = mcp --no-index（只读共享索引）',
    JSON.stringify(mcpEntry?.args) === JSON.stringify(['mcp', '--no-index']), JSON.stringify(mcpEntry?.args))
  check('stdio entry 不带 url/headers 字段（mcp-manager 校验规则）',
    mcpEntry?.url === undefined && mcpEntry?.headers === undefined)
  // config 开关与透传：mcp=false 不注册；mcpCommand/mcpArgs 原样透传进 entry。
  registered.mcp.clear()
  apply(rootCtx, { endpoint: MOCK_ENDPOINT, mcp: false })
  check('config.mcp=false 跳过注册', !registered.mcp.has('anyfind'))
  apply(rootCtx, { endpoint: MOCK_ENDPOINT, mcpCommand: 'X:/bin/anyfindd.exe', mcpArgs: ['mcp'] })
  const mcpCustom = registered.mcp.get('anyfind')
  check('mcpCommand/mcpArgs 透传（显式路径不探测存在）',
    mcpCustom?.command === 'X:/bin/anyfindd.exe' && JSON.stringify(mcpCustom?.args) === JSON.stringify(['mcp']))
  // 回到默认注册，后续段落沿用默认 entry。
  apply(rootCtx, { endpoint: MOCK_ENDPOINT })
  check('恢复默认注册（缺省 args 回到 --no-index）',
    JSON.stringify(registered.mcp.get('anyfind')?.args) === JSON.stringify(['mcp', '--no-index']))

  console.log('▶ anyfind_search 工具（内核在线）：')
  const toolOut = await tool.execute({ query: 'nginx' }, {})
  check('返回命中摘要', typeof toolOut === 'string' && toolOut.includes('命中'), String(toolOut).slice(0, 80))
  check('摘要含结构化字段（agent/项目/session_id）',
    toolOut.includes('claude-code') && toolOut.includes('session_id') && toolOut.includes('sess-abc-123'))
  check('摘要无 <mark> 残留（模型面纯文本）', !toolOut.includes('<mark>'))
  const toolFiltered = await tool.execute({ query: 'nginx', kinds: 'session', limit: 5 }, {})
  check('kinds 过滤生效（仅会话组）', toolFiltered.includes('会话') && !toolFiltered.includes('[文件]'))
  const toolEmpty = await tool.execute({ query: '' }, {})
  check('空 query 友好报错', toolEmpty.includes('query 不能为空'))

  console.log('▶ /anyfind 斜杠命令：')
  const cmd = registered.commands.get('anyfind')
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
  const health = await callRoute('/dsh-anyfind/api/health')
  check('GET health 200 且透传索引计数', health.statusCode === 200 && health.json().index.sessions === 86)
  const search = await callRoute('/dsh-anyfind/api/search', { query: 'q=nginx&kinds=session' })
  check('GET search 透传 groups', search.statusCode === 200 && search.json().groups.session.length === 2)
  const searchEmpty = await callRoute('/dsh-anyfind/api/search', { query: 'q=' })
  check('GET search 空 q → 400', searchEmpty.statusCode === 400)
  const resume = await callRoute('/dsh-anyfind/api/resume', {
    method: 'POST',
    body: JSON.stringify({ agent: 'claude-code', session_id: 'sess-abc-123', cwd: 'D:/coding/filefind/dsh-plugin' }),
    headers: {},
  })
  check('POST resume 返回命令串', resume.statusCode === 200 && resume.json().command === 'claude --resume sess-abc-123', resume.body)
  const cfgGet = await callRoute('/dsh-anyfind/api/config')
  check('GET config 返回当前 endpoint', cfgGet.statusCode === 200 && cfgGet.json().endpoint === MOCK_ENDPOINT)
  const cfgBad = await callRoute('/dsh-anyfind/api/config', { method: 'POST', body: JSON.stringify({ endpoint: 'not a url' }) })
  check('POST config 非法地址 → 400', cfgBad.statusCode === 400)

  console.log('▶ 文件端点代理（契约 v0.2 透传）：')
  const q = (params) => new URLSearchParams(params).toString()
  const listOk = await callRoute('/dsh-anyfind/api/file/list', {
    query: q({ path: 'D:/coding/anyfind/apps/dsh-plugin' }),
  })
  check('GET file/list 200 且回显 path',
    listOk.statusCode === 200 && listOk.json().path === 'D:/coding/anyfind/apps/dsh-plugin', listOk.body)
  const entries = listOk.statusCode === 200 ? listOk.json().entries : []
  check('GET file/list 目录优先、同组按名排序',
    entries.length >= 4 && entries.slice(0, 3).every((e) => e.is_dir === true)
      && entries.slice(0, 3).map((e) => e.name).join(',') === 'lib,src,test', JSON.stringify(entries.map((e) => e.name)))
  check('GET file/list 默认隐藏噪声目录',
    !entries.some((e) => e.name === 'node_modules' || e.name === '.git'))
  check('GET file/list 文件条目结构（name/size/mtime）',
    entries.some((e) => e.name === 'package.json' && e.is_dir === false && e.size === 2048 && e.mtime === '2026-11-01T11:05:00'))
  const listNoise = await callRoute('/dsh-anyfind/api/file/list', {
    query: q({ path: 'D:/coding/anyfind/apps/dsh-plugin', noise: '1' }),
  })
  check('noise=1 透传（噪声目录可见）',
    listNoise.statusCode === 200 && listNoise.json().entries.some((e) => e.name === 'node_modules'))
  const listMax = await callRoute('/dsh-anyfind/api/file/list', {
    query: q({ path: 'D:/coding/anyfind/apps/dsh-plugin', max: '2' }),
  })
  check('max 截断透传（truncated=true）',
    listMax.statusCode === 200 && listMax.json().entries.length === 2 && listMax.json().truncated === true)

  const statOk = await callRoute('/dsh-anyfind/api/file/stat', {
    query: q({ path: 'D:/coding/anyfind/apps/dsh-plugin/package.json' }),
  })
  check('GET file/stat 200 透传 name/size/mtime',
    statOk.statusCode === 200 && statOk.json().name === 'package.json'
      && statOk.json().is_dir === false && statOk.json().size === 2048, statOk.body)
  const statMissing = await callRoute('/dsh-anyfind/api/file/stat', {
    query: q({ path: 'D:/no/such/file.txt' }),
  })
  check('GET file/stat 不存在 → 404 透传',
    statMissing.statusCode === 404 && typeof statMissing.json().error === 'string')

  console.log('▶ 盘符端点（file/drives 透传，契约 v0.2 2026-11-21 补充）：')
  const drivesOk = await callRoute('/dsh-anyfind/api/file/drives')
  const drives = drivesOk.statusCode === 200 ? drivesOk.json().drives : []
  check('GET file/drives 200 透传 drives 数组', drivesOk.statusCode === 200 && Array.isArray(drives), drivesOk.body)
  check('file/drives 条目结构（path "X:/" + type 枚举）',
    drives.length === 5 && drives[0].path === 'C:/' && drives[0].type === 'fixed'
      && drives.some((d) => d.type === 'removable') && drives.some((d) => d.type === 'cdrom')
      && drives.some((d) => d.type === 'unknown'), drivesOk.body)
  check('file/drives 按盘符排序（C ≤ D ≤ E ≤ F ≤ Z）',
    drives.map((d) => d.path).join(',') === 'C:/,D:/,E:/,F:/,Z:/', drivesOk.body)

  const listNoPath = await callRoute('/dsh-anyfind/api/file/list')
  check('非法参数：缺 path → 400 + error',
    listNoPath.statusCode === 400 && typeof listNoPath.json().error === 'string', listNoPath.body)
  const listRelative = await callRoute('/dsh-anyfind/api/file/list', { query: 'path=relative/dir' })
  check('非法参数：相对路径 → 400 透传',
    listRelative.statusCode === 400 && typeof listRelative.json().error === 'string', listRelative.body)

  console.log('▶ 文件写操作（file/op 透传 + 批量上限纯函数）：')
  const opGet = await callRoute('/dsh-anyfind/api/file/op', { method: 'GET' })
  check('file/op 非 POST → 405', opGet.statusCode === 405)
  const postOp = (body) => callRoute('/dsh-anyfind/api/file/op', {
    method: 'POST',
    body: JSON.stringify(body),
    headers: {},
  })

  const opOk = await postOp({
    op: 'rename',
    paths: ['D:/coding/anyfind/apps/dsh-plugin/README.md'],
    target: 'D:/coding/anyfind/apps/dsh-plugin',
    newName: 'README2.md',
  })
  check('POST op 成功态透传 200 {ok,results}',
    opOk.statusCode === 200 && opOk.json().ok === true && opOk.json().results.length === 1
      && opOk.json().results[0].ok === true
      && opOk.json().results[0].path === 'D:/coding/anyfind/apps/dsh-plugin/README.md', opOk.body)

  const opMixed = await postOp({
    op: 'recycle',
    paths: ['D:/coding/anyfind/apps/dsh-plugin/src/client.ts', 'D:/coding/anyfind/apps/dsh-plugin/locked.txt'],
  })
  check('POST op 单 path 失败态：200 + results[1].ok=false + error',
    opMixed.statusCode === 200 && opMixed.json().ok === true && opMixed.json().results.length === 2
      && opMixed.json().results[0].ok === true && opMixed.json().results[1].ok === false
      && typeof opMixed.json().results[1].error === 'string'
      && opMixed.json().results[1].error.includes('拒绝访问'), opMixed.body)

  const opTooMany = await postOp({
    op: 'copy',
    paths: Array.from({ length: 129 }, (_, i) => `D:/coding/x${i}.txt`),
    target: 'D:/coding',
  })
  check('POST op paths 超 128 → 400 透传',
    opTooMany.statusCode === 400 && typeof opTooMany.json().error === 'string'
      && opTooMany.json().error.includes('128'), opTooMany.body)

  const opBadJson = await callRoute('/dsh-anyfind/api/file/op', {
    method: 'POST',
    body: '{oops',
    headers: {},
  })
  check('POST op 非法 JSON → 400 透传',
    opBadJson.statusCode === 400 && typeof opBadJson.json().error === 'string', opBadJson.body)

  const opNoTarget = await postOp({ op: 'cut', paths: ['D:/coding/a.txt'] })
  check('POST op cut 缺 target → 400 透传',
    opNoTarget.statusCode === 400 && String(opNoTarget.json().error).includes('target'), opNoTarget.body)

  console.log('▶ 批量上限参数校验（纯函数）：')
  const paths128 = Array.from({ length: FILE_OP_MAX_PATHS }, (_, i) => `D:/coding/x${i}.txt`)
  check('FILE_OP_MAX_PATHS === 128（契约）', FILE_OP_MAX_PATHS === 128)
  check('validateFileOpRequest 128 条通过', validateFileOpRequest({ op: 'recycle', paths: paths128 }) === null)
  const overErr = validateFileOpRequest({ op: 'recycle', paths: [...paths128, 'D:/coding/one-more.txt'] })
  check('validateFileOpRequest 129 条报超限',
    typeof overErr === 'string' && overErr.includes('128'), String(overErr))
  check('validateFileOpRequest 空 paths 报错',
    typeof validateFileOpRequest({ op: 'recycle', paths: [] }) === 'string')
  check('validateFileOpRequest op 白名单',
    typeof validateFileOpRequest({ op: 'delete', paths: ['D:/a'] }) === 'string')
  check('validateFileOpRequest copy 缺 target 报错',
    String(validateFileOpRequest({ op: 'copy', paths: ['D:/a'] })).includes('target'))
  check('validateFileOpRequest rename 缺 newName 报错',
    String(validateFileOpRequest({ op: 'rename', paths: ['D:/a'], target: 'D:/' })).includes('newName'))
  check('validateFileOpRequest newDir 单条约束',
    typeof validateFileOpRequest({ op: 'newDir', paths: ['D:/a', 'D:/b'], newName: 'x' }) === 'string')
  check('validateFileOpRequest 合法 rename 通过',
    validateFileOpRequest({ op: 'rename', paths: ['D:/a/old.txt'], target: 'D:/a', newName: 'new.txt' }) === null)

  console.log('▶ 文件读取（file/read 分页透传）：')
  const PLUGIN_ROOT = 'D:/coding/anyfind/apps/dsh-plugin'
  const readMid = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/README.md`, offset: '30', lines: '60' }),
  })
  check('GET file/read 200 窗口分页（offset 30 / 60 行 / total 120）',
    readMid.statusCode === 200 && readMid.json().offset === 30 && readMid.json().lines === 60
      && readMid.json().totalLines === 120 && readMid.json().truncated === false
      && readMid.json().binary === false && readMid.json().content.split('\n').length === 60, readMid.body)
  const readTail = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/README.md`, offset: '100', lines: '60' }),
  })
  check('file/read 末页截断标记（truncated=true，余 20 行）',
    readTail.statusCode === 200 && readTail.json().lines === 20 && readTail.json().truncated === true, readTail.body)
  const readDefault = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/notes.txt` }),
  })
  check('file/read 缺省 30 行窗口',
    readDefault.statusCode === 200 && readDefault.json().offset === 0 && readDefault.json().lines === 2
      && readDefault.json().truncated === true, readDefault.body)
  const readBinary = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/logo.png` }),
  })
  check('file/read 二进制 → { binary: true } 且无 content',
    readBinary.statusCode === 200 && readBinary.json().binary === true && !('content' in readBinary.json()), readBinary.body)
  const readHuge = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/huge.txt` }),
  })
  check('file/read >2MB → 413 透传', readHuge.statusCode === 413 && typeof readHuge.json().error === 'string', readHuge.body)
  const readNoPath = await callRoute('/dsh-anyfind/api/file/read')
  check('file/read 缺 path → 400', readNoPath.statusCode === 400 && typeof readNoPath.json().error === 'string')
  const readMissing = await callRoute('/dsh-anyfind/api/file/read', { query: q({ path: 'D:/no/such.txt' }) })
  check('file/read 不存在 → 404 透传', readMissing.statusCode === 404 && typeof readMissing.json().error === 'string')

  console.log('▶ 文件读取（format=base64 字节通路）：')
  const b64Png = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/logo.png`, format: 'base64' }),
  })
  check('GET file/read?format=base64 二进制 → 200 content_base64',
    b64Png.statusCode === 200 && typeof b64Png.json().content_base64 === 'string'
      && b64Png.json().content_base64.length > 0, b64Png.body)
  const pngBytes = Buffer.from(b64Png.json().content_base64 ?? '', 'base64')
  check('content_base64 解码 = PNG magic（89504E47）',
    pngBytes.subarray(0, 4).toString('hex') === '89504e47', pngBytes.subarray(0, 4).toString('hex'))
  const b64Text = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/README.md`, format: 'base64' }),
  })
  const textBytes = Buffer.from(b64Text.json().content_base64 ?? '', 'base64').toString('utf8')
  check('format=base64 文本文件同样可用（UTF-8 还原；契约字段与内核同形，无 size）',
    b64Text.statusCode === 200 && textBytes.startsWith('第 1 行：')
      && b64Text.json().size === undefined && b64Text.json().truncated === false, b64Text.body)
  const b64Pdf = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/slide.pdf`, format: 'base64' }),
  })
  const pdfBytes = Buffer.from(b64Pdf.json().content_base64 ?? '', 'base64')
  check('format=base64 PDF 夹具以 %PDF- 开头（可喂 pdfjs）',
    b64Pdf.statusCode === 200 && pdfBytes.subarray(0, 5).toString('latin1') === '%PDF-', b64Pdf.body)
  const b64Huge = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/huge.txt`, format: 'base64' }),
  })
  check('format=base64 >2MB → 413', b64Huge.statusCode === 413 && typeof b64Huge.json().error === 'string', b64Huge.body)
  const b64Bad = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: `${PLUGIN_ROOT}/README.md`, format: 'bogus' }),
  })
  check('format 非法值 → 400 透传', b64Bad.statusCode === 400 && typeof b64Bad.json().error === 'string', b64Bad.body)
  const b64Missing = await callRoute('/dsh-anyfind/api/file/read', {
    query: q({ path: 'D:/no/such.txt', format: 'base64' }),
  })
  check('format=base64 不存在 → 404 透传',
    b64Missing.statusCode === 404 && typeof b64Missing.json().error === 'string', b64Missing.body)
  check('不传 format 行为不变（logo.png 仍 binary:true 且无 content_base64）',
    readBinary.statusCode === 200 && readBinary.json().binary === true
      && readBinary.json().content_base64 === undefined)

  console.log('▶ 预览字节端点（file/bytes，面板专用，直读磁盘不走内核）：')
  const bytesOk = await callRoute('/dsh-anyfind/api/file/bytes', {
    query: q({ path: `${PLUGIN_ROOT}/README.md` }),
  })
  const realReadme = readFileSync(`${PLUGIN_ROOT}/README.md`, 'utf8')
  // sendBinary 走 Buffer：fakeRes 二进制保留在 .buffer（body 仅文本响应有值）。
  check('file/bytes 200 且字节与磁盘一致',
    bytesOk.statusCode === 200 && bytesOk.buffer !== null
      && bytesOk.buffer.toString('utf8') === realReadme, `status=${bytesOk.statusCode}`)
  const bytesMissing = await callRoute('/dsh-anyfind/api/file/bytes', { query: q({ path: 'D:/no/such.bin' }) })
  check('file/bytes 不存在 → 404 + error', bytesMissing.statusCode === 404 && typeof bytesMissing.json().error === 'string')
  const bytesRelative = await callRoute('/dsh-anyfind/api/file/bytes', { query: 'path=relative/file.bin' })
  check('file/bytes 相对路径 → 400 + error', bytesRelative.statusCode === 400 && typeof bytesRelative.json().error === 'string')
  const bytesDir = await callRoute('/dsh-anyfind/api/file/bytes', { query: q({ path: `${PLUGIN_ROOT}/src` }) })
  check('file/bytes 目录 → 400 + error', bytesDir.statusCode === 400 && typeof bytesDir.json().error === 'string')

  console.log('▶ office→PDF 转换端点（officeKit/officeToPdf 能力降级）：')
  const officeBadExt = await callRoute('/dsh-anyfind/api/office/pdf', {
    query: q({ path: `${PLUGIN_ROOT}/README.md` }),
  })
  check('office/pdf 非 office 扩展 → 400 + error',
    officeBadExt.statusCode === 400 && typeof officeBadExt.json().error === 'string', officeBadExt.body)
  // 假 ctx 无 officeKit 服务（get → undefined）：能力缺席降级 503。
  const officeNoKit = await callRoute('/dsh-anyfind/api/office/pdf', {
    query: q({ path: `${PLUGIN_ROOT}/README.docx` }),
  })
  check('office/pdf officeKit 未挂载 → 503 + code=unavailable',
    officeNoKit.statusCode === 503 && officeNoKit.json().code === 'unavailable'
      && typeof officeNoKit.json().error === 'string', officeNoKit.body)

  const searchFiles = await callRoute('/dsh-anyfind/api/search', { query: 'q=nginx&kinds=file&limit=50' })
  check('search kinds=file file 组 mock（@ 触发源候选源）',
    searchFiles.statusCode === 200 && searchFiles.json().groups.file.length === 3
      && searchFiles.json().groups.file.every((h) => typeof h.path === 'string' && h.path !== ''), searchFiles.body)

  console.log('▶ 引用格式化 / 目录树文本（纯函数）：')
  check('formatReference 单条原样', formatReference(['D:/coding/a.txt']) === 'D:/coding/a.txt')
  check('formatReference 多条按行拼接并去空白',
    formatReference([' D:/a ', '', 'D:/b']) === 'D:/a\nD:/b')
  check('formatReference 空输入为空串', formatReference([]) === '')
  const treeText = buildTreeText([{
    name: 'D:/dir',
    children: [
      { name: 'a.txt' },
      { name: 'sub', children: [{ name: 'b.txt' }, { name: 'c.txt' }] },
    ],
  }])
  const treeLines = treeText.split('\n')
  check('文本树根行', treeLines[0] === 'D:/dir')
  check('文本树 ├─/└─ 连接符', treeText.includes('├─ a.txt') && treeText.includes('└─ sub'))
  check('文本树子层缩进', treeText.includes('   ├─ b.txt') && treeText.includes('   └─ c.txt'))
  const branchTree = buildTreeText([{
    name: 'r',
    children: [
      { name: 'd1', children: [{ name: 'y' }] },
      { name: 'd2' },
    ],
  }])
  check('文本树非末位父节点用 │ 续接', branchTree.includes('│  └─ y') && branchTree.includes('└─ d2'),
    JSON.stringify(branchTree))
  const deepTree = buildTreeText([{
    name: 'root',
    children: Array.from({ length: 300 }, (_, i) => ({ name: `f${i}.txt` })),
  }]).split('\n')
  check('文本树 ≤200 行截断并附说明',
    deepTree.length === TREE_MAX_LINES && TREE_MAX_LINES === 200
      && deepTree[deepTree.length - 1].includes('已截断'), String(deepTree.length))
  const nestedTree = buildTreeText([{
    name: 'r',
    children: [{ name: 'd1', children: [{ name: 'd2', children: [{ name: 'x' }] }] }],
  }])
  check('文本树多层缩进续接', nestedTree.includes('└─ d1')
    && nestedTree.includes('   └─ d2') && nestedTree.includes('      └─ x'), JSON.stringify(nestedTree))

  console.log('▶ 收藏夹纯函数（文件工作台①：localStorage(anyfind.favorites) 载荷约定）：')
  check('FAVORITES_KEY === anyfind.favorites（任务约定键）', FAVORITES_KEY === 'anyfind.favorites')
  const favA = { path: 'D:/coding/a', name: 'a', isDir: true, addedAt: 1 }
  const favB = { path: 'D:/coding/b.txt', name: 'b.txt', isDir: false, addedAt: 2 }
  const favC = { path: 'D:/coding/c', name: 'c', isDir: true, addedAt: 3 }
  let favList = favoritesAdd(favoritesAdd(favoritesAdd([], favA), favB), favC)
  check('favoritesAdd 头插且保序', favList.map((x) => x.name).join(',') === 'c,b.txt,a')
  check('favoritesAdd 去重（已收藏返回同一引用）', favoritesAdd(favList, favA) === favList)
  check('favoritesHas 命中（trim 后比较）', favoritesHas(favList, ' D:/coding/b.txt ') === true
    && favoritesHas(favList, 'D:/missing') === false)
  check('favoritesRemove 移除', favoritesRemove(favList, 'D:/coding/a').map((x) => x.name).join(',') === 'c,b.txt')
  check('favoritesRemove 缺席返回同一引用', favoritesRemove(favList, 'D:/missing') === favList)
  check('favoritesToggle 在册移除/不在册收藏',
    favoritesToggle(favList, favB).map((x) => x.name).join(',') === 'c,a'
    && favoritesToggle(favoritesRemove(favList, favB.path), favB).map((x) => x.name).join(',') === 'b.txt,c,a')
  check('favoritesMove 拖拽排序 0→2', favoritesMove(favList, 0, 2).map((x) => x.name).join(',') === 'b.txt,a,c')
  check('favoritesMove 非法/原位返回同一引用',
    favoritesMove(favList, -1, 0) === favList && favoritesMove(favList, 0, 0) === favList
      && favoritesMove([], 0, 1).length === 0)
  check('FAVORITES_MAX === 200 且 add 截断丢最旧',
    FAVORITES_MAX === 200 && (() => {
      let many = []
      for (let i = 0; i < 205; i++) many = favoritesAdd(many, { path: `D:/f${i}`, name: `f${i}`, isDir: false, addedAt: i })
      return many.length === 200 && many[0].path === 'D:/f204' && many[199].path === 'D:/f5'
    })())
  check('normalizeFavorites 过滤畸形项 + 去重 + 截断',
    (() => {
      const good = { path: 'D:/x', name: 'x', isDir: false, addedAt: 9 }
      const out = normalizeFavorites(['junk', null, 5, good, { ...good }, { path: '  ', name: 'blank', isDir: false, addedAt: 1 }])
      return out.length === 1 && out[0].path === 'D:/x'
    })())
  check('normalizeFavorites 非 JSON 数组载荷 → 空列表',
    normalizeFavorites(null).length === 0 && normalizeFavorites('{bad').length === 0)
  check('盘符类型 label 覆盖契约五枚举',
    Object.keys(DRIVE_TYPE_LABEL).join(',') === 'fixed,removable,cdrom,ram,unknown')

  console.log('▶ 压缩包浏览（文件工作台④：bytes 端点 → vendor archive 解析/条目格式化）：')
  // 最小 ZIP 构造器：stored(method 0) + deflate(method 8) 条目，真实 CRC32
  // （readZipEntry 默认 verifyCrc，伪造 CRC 会挂"校验不一致"备注）。
  const CRC_TABLE = (() => {
    const t = new Uint32Array(256)
    for (let i = 0; i < 256; i++) {
      let c = i
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
      t[i] = c >>> 0
    }
    return t
  })()
  function crc32Of(buf) {
    let c = 0xffffffff
    for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8)
    return (c ^ 0xffffffff) >>> 0
  }
  const u16 = (n) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b }
  const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n >>> 0); return b }
  function buildZip(entries) {
    const locals = []
    const centrals = []
    let offset = 0
    for (const e of entries) {
      const nameB = Buffer.from(e.dir === true ? `${e.name}/` : e.name, 'utf8')
      const raw = e.dir === true ? Buffer.alloc(0) : Buffer.from(e.data, 'utf8')
      const method = e.method ?? 0
      const comp = method === 8 ? deflateRawSync(raw) : raw
      const crc = crc32Of(raw)
      const local = Buffer.concat([
        u32(0x04034b50), u16(20), u16(0), u16(method), u16(0), u16(0), u32(crc),
        u32(comp.length), u32(raw.length), u16(nameB.length), u16(0), nameB, comp,
      ])
      locals.push(local)
      centrals.push(Buffer.concat([
        u32(0x02014b50), u16(20), u16(20), u16(0), u16(method), u16(0), u16(0), u32(crc),
        u32(comp.length), u32(raw.length), u16(nameB.length), u16(0), u16(0), u16(0), u16(0),
        u32(0), u32(offset), nameB,
      ]))
      offset += local.length
    }
    const cd = Buffer.concat(centrals)
    const eocd = Buffer.concat([
      u32(0x06054b50), u16(0), u16(0), u16(entries.length), u16(entries.length),
      u32(cd.length), u32(offset), u16(0),
    ])
    return Buffer.concat([...locals, cd, eocd])
  }
  const DEFLATED_TEXT = 'deflate payload 行'.repeat(50)
  const zipBuf = buildZip([
    { name: 'docs', dir: true },
    { name: 'docs/readme.txt', data: 'archive readme line1\nline2\nline3' },
    { name: 'src/main.js', data: 'console.log("hi")' },
    { name: 'src/deflated.txt', data: DEFLATED_TEXT, method: 8 },
  ])
  const archDir = mkdtempSync(join(tmpdir(), 'anyfind-arch-'))
  const zipPath = join(archDir, 'fixture.zip').replace(/\\/g, '/')
  writeFileSync(zipPath, zipBuf)
  const archBytesRes = await callRoute('/dsh-anyfind/api/file/bytes', { query: q({ path: zipPath }) })
  check('file/bytes 取回 ZIP 字节（与构造字节一致）',
    archBytesRes.statusCode === 200 && archBytesRes.buffer !== null
      && Buffer.compare(archBytesRes.buffer, zipBuf) === 0, `status=${archBytesRes.statusCode}`)
  const arch = await import('../src/vendor/archive/index.js')
  const zipBytes = new Uint8Array(archBytesRes.buffer.buffer, archBytesRes.buffer.byteOffset, archBytesRes.buffer.byteLength)
  const archList = await arch.listArchive(zipBytes, { filename: 'fixture.zip' })
  check('listArchive(zip) 识别 format=zip 且 4 条目',
    archList.supported === true && archList.format === 'zip' && archList.entries.length === 4,
    JSON.stringify({ supported: archList.supported, format: archList.format, n: archList.entries.length }))
  const archPaths = archList.entries.map((e) => e.path).join(',')
  check('条目路径含目录项（vendor normalize 去尾 /）与嵌套文件',
    archPaths === 'docs,docs/readme.txt,src/main.js,src/deflated.txt'
      && archList.entries[0].dir === true, archPaths)
  const readmeEntry = archList.entries.find((e) => e.path === 'docs/readme.txt')
  const deflatedEntry = archList.entries.find((e) => e.path === 'src/deflated.txt')
  const readmeText = await arch.readEntryText(zipBytes, readmeEntry)
  check('readEntryText stored 条目还原文本（method 0）',
    readmeText.text === 'archive readme line1\nline2\nline3', JSON.stringify(readmeText.text))
  const deflatedText = await arch.readEntryText(zipBytes, deflatedEntry)
  check('readEntryText deflate 条目还原（DecompressionStream deflate-raw，method 8）',
    deflatedText.text === DEFLATED_TEXT, `${deflatedText.text.length} vs ${DEFLATED_TEXT.length}`)
  const archTree = arch.buildTree(archList.entries)
  check('buildTree 目录树（docs/src 两支，子条目按码点排序挂靠）',
    archTree.children.map((c) => c.name).join(',') === 'docs,src'
      && archTree.children[0].children[0].name === 'readme.txt'
      && archTree.children[1].children.map((c) => c.name).join(',') === 'deflated.txt,main.js',
    JSON.stringify(archTree.children.map((c) => [c.name, c.children?.map((x) => x.name)])))
  check('formatArchiveEntry 文件条目（名字/大小/方法）',
    formatArchiveEntry({ path: 'src/main.js', dir: false, size: 19, method: 'stored' }) === 'src/main.js  19 B  stored',
    formatArchiveEntry({ path: 'src/main.js', dir: false, size: 19, method: 'stored' }))
  check('formatArchiveEntry 目录条目（尾 / 与 — 占位）',
    formatArchiveEntry({ path: 'docs', dir: true }) === 'docs/  —')
  check('formatArchiveEntry 加密条目带标记',
    formatArchiveEntry({ path: 'e.bin', dir: false, size: 5, method: 'deflate', encrypted: true }).endsWith('·已加密'))
  check('formatArchiveEntryReference 格式 path!/entry（按钮与拖拽共用）',
    formatArchiveEntryReference('D:/tmp/fixture.zip', 'docs/readme.txt') === 'D:/tmp/fixture.zip!/docs/readme.txt')
  check('formatArchiveEntryReference 去首斜杠/空串防御',
    formatArchiveEntryReference('D:/a.zip', '/abs/path') === 'D:/a.zip!/abs/path'
      && formatArchiveEntryReference('', 'x') === '' && formatArchiveEntryReference('D:/a.zip', '/') === '')
  rmSync(archDir, { recursive: true, force: true })

  console.log('▶ 划词引用格式化（文件工作台⑥：selectionToLines / 行号 / 页码引用）：')
  const doc = 'l1\nl2\nl3\nl4\nl5'
  const selMid = selectionToLines(doc, 3, 8, 1)
  check('selectionToLines 偏移→行号区间（1-based，含首行号窗口起点）',
    selMid !== null && selMid.start === 2 && selMid.end === 3 && selMid.text === 'l2\nl3', JSON.stringify(selMid))
  check('selectionToLines collapsed → null', selectionToLines(doc, 3, 3, 1) === null)
  const selAll = selectionToLines(doc, -5, 99, 5)
  check('selectionToLines 偏移双向 clamp（分页窗口起始行 5）',
    selAll !== null && selAll.start === 5 && selAll.end === 9 && selAll.text === doc, JSON.stringify(selAll))
  check('formatLineReference 路径:起-止 + 首行摘要',
    formatLineReference('D:/coding/a.ts', 2, 3, 'l2\nl3') === 'D:/coding/a.ts:2-3\n> l2')
  check('formatLineReference 摘要跳过空行取第一个非空行',
    formatLineReference('p.md', 1, 2, '\n   \nsecond line') === 'p.md:1-2\n> second line')
  check('formatLineReference 空选区/空白选区 → 空串',
    formatLineReference('p', 1, 1, '') === '' && formatLineReference('p', 1, 1, '  \n ') === '')
  check('formatLineReference 超长摘要截断 160 字 + …',
    formatLineReference('p', 1, 1, `${'x'.repeat(200)}\nrest`) === `p:1-1\n> ${'x'.repeat(160)}…`)
  check('formatPageReference 路径:第 N 页 + 首行摘要',
    formatPageReference('D:/s/slide.pdf', 3, '标题行\n正文') === 'D:/s/slide.pdf:第 3 页\n> 标题行')
  check('formatPageReference 空选区 → 空串', formatPageReference('p.pdf', 1, '   ') === '')


  console.log('▶ 客户端产物工厂（lib/client.js：vendor 预览 react 注入求值）：')
  const clientCode = readFileSync(join(ROOT, 'lib', 'client.js'), 'utf8')
  let clientFactory = null
  new Function('window', clientCode)({
    __ModuleLoader__: { load: (def) => { clientFactory = def } },
  })
  check('lib/client.js 注册 __ModuleLoader__ 工厂',
    clientFactory !== null && typeof clientFactory.factory === 'function')
  if (clientFactory !== null) {
    const req = createRequire(import.meta.url)
    let clientExports = null
    let factoryError = null
    try {
      clientExports = clientFactory.factory((name) => {
        if (name === 'react') return req('react')
        if (name === 'react/jsx-runtime') return req('react/jsx-runtime')
        throw new Error(`工厂出现意外裸引用：${name}`)
      })
    } catch (e) {
      factoryError = e
    }
    // vendor 模块（src/vendor/ui）顶层执行 const h = react.createElement：
    // 求值通过即证明 build.mjs 的 var react 工厂注入生效。
    check('工厂求值通过（vendor 顶层 react 注入生效，导出 apply）',
      factoryError === null && clientExports !== null && typeof clientExports.apply === 'function',
      factoryError !== null ? String(factoryError) : '')
  }

  console.log('▶ 内核 down 降级：')
  mock.kill()
  await delay(300)
  const healthDown = await callRoute('/dsh-anyfind/api/health')
  check('health → 502 + down 标记 + 引导', healthDown.statusCode === 502 && healthDown.json().down === true && healthDown.json().hint.includes('AnyFind 桌面版'))
  const listDown = await callRoute('/dsh-anyfind/api/file/list', { query: q({ path: 'D:/coding' }) })
  check('file/list → 502 + down 标记', listDown.statusCode === 502 && listDown.json().down === true && listDown.json().hint.includes('AnyFind 桌面版'))
  const toolDown = await tool.execute({ query: 'nginx' }, {})
  check('工具返回友好错误而非抛出', typeof toolDown === 'string' && toolDown.includes('内核不可达') && toolDown.includes('未启动'))
  const cmdDown = await cmd.handler({ rawInput: ' nginx', signal: new AbortController().signal })
  check('斜杠命令返回友好错误', cmdDown.kind === 'error' && cmdDown.text.includes('未启动'))
} finally {
  mock.kill()
}

console.log(`\n${failed === 0 ? '✅' : '❌'} selftest: ${passed} passed, ${failed} failed`)
process.exit(failed === 0 ? 0 : 1)
