// mock-core.mjs — 按 docs/api-contract.md（搜索组 v0.1 / 文件组 v0.2）返回假数据的统一搜索内核 mock。
// 用法：node test/mock-core.mjs [port]（默认 18765，避免与真内核 8765 撞车）
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const port = Number(process.argv[2] ?? '18765')

const HITS = {
  file: [
    { kind: 'file', title: 'nginx.conf', path: 'C:/Users/me/filefind/nginx.conf', snippet: null, mtime: '2026-10-12T08:30:00', agent: null, project: 'filefind', session_id: null, cwd: null, score: 0.91 },
    { kind: 'file', title: 'client.ts', path: 'C:/Users/me/anyfind/apps/dsh-plugin/src/client.ts', snippet: '面板里加 <mark>预览</mark> 与引用按钮', mtime: '2026-11-13T09:20:00', agent: null, project: 'dsh-plugin', session_id: null, cwd: null, score: 0.88 },
    { kind: 'file', title: 'api-contract.md', path: 'C:/Users/me/anyfind/docs/api-contract.md', snippet: '文件端点契约 v0.2 <mark>file/read</mark> 分页', mtime: '2026-11-12T17:05:00', agent: null, project: 'anyfind', session_id: null, cwd: null, score: 0.81 },
  ],
  content: [
    { kind: 'content', title: '部署笔记.md', path: 'C:/Users/me/notes/deploy.md', snippet: '反代配置 <mark>nginx</mark> 的 server_name 要带上内网域名', mtime: '2026-10-10T14:02:00', agent: null, project: null, session_id: null, cwd: null, score: 0.77 },
  ],
  session: [
    { kind: 'session', title: '给 dsh 插件加搜索面板', path: null, snippet: '用户：之前 <mark>nginx</mark> 反代怎么配的来着？\n助手：上次的配置在…', mtime: '2026-10-18T03:12:00', agent: 'claude-code', project: 'dsh-plugin', session_id: 'sess-abc-123', cwd: 'C:/Users/me/filefind/dsh-plugin', score: 0.83 },
    { kind: 'session', title: '内核索引联调', path: null, snippet: '<mark>nginx</mark> access log 解析', mtime: '2026-10-15T09:40:00', agent: 'codex', project: 'filefind', session_id: 'sess-def-456', cwd: 'C:/Users/me/filefind', score: 0.66 },
  ],
  skill: [
    { kind: 'skill', title: 'opencli-adapter-author', path: 'C:/Users/x/.agents/skills/opencli-adapter-author/SKILL.md', snippet: '为 OpenCLI 编写新站点 <mark>nginx</mark> 适配器…', mtime: '2026-09-30T11:00:00', agent: 'claude-code', project: null, session_id: null, cwd: null, score: 0.52 },
  ],
  memory: [
    { kind: 'memory', title: 'CLAUDE.md', path: 'C:/Users/me/filefind/CLAUDE.md', snippet: '本项目反代统一走 <mark>nginx</mark>，不用 caddy', mtime: '2026-10-01T02:00:00', agent: 'claude-code', project: 'filefind', session_id: null, cwd: null, score: 0.71 },
  ],
}

function send(res, code, body) {
  res.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'access-control-allow-origin': '*' })
  res.end(JSON.stringify(body))
}

// 文件端点夹具（契约 v0.2）：噪声目录默认隐藏、目录优先按名排序由 mock 复刻。
const NOISE_DIRS = new Set(['.git', 'node_modules', 'dist', 'build', 'coverage', '.idea', 'target'])
// 夹具根 = 本文件（test/）的上一级插件根，任何克隆位置都成立。
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..').replace(/\\/g, '/')
const ROOT_PARENT = ROOT.slice(0, ROOT.lastIndexOf('/'))
const DIR_MTIME = '2026-11-01T10:00:00'
const ROOT_ENTRIES = [
  { name: '.git', is_dir: true, size: 0, mtime: '2026-11-01T09:00:00' },
  { name: 'node_modules', is_dir: true, size: 0, mtime: '2026-11-01T09:30:00' },
  { name: 'lib', is_dir: true, size: 0, mtime: '2026-11-01T10:05:00' },
  { name: 'src', is_dir: true, size: 0, mtime: '2026-11-01T10:10:00' },
  { name: 'test', is_dir: true, size: 0, mtime: '2026-11-01T10:15:00' },
  { name: 'README.md', is_dir: false, size: 1024, mtime: '2026-11-01T11:00:00' },
  { name: 'package.json', is_dir: false, size: 2048, mtime: '2026-11-01T11:05:00' },
  { name: 'tsconfig.json', is_dir: false, size: 512, mtime: '2026-11-01T11:10:00' },
]
const KNOWN_FILES = {
  [`${ROOT}/package.json`]: { name: 'package.json', is_dir: false, size: 2048, mtime: '2026-11-01T11:05:00' },
}

// /api/file/read 夹具：120 行文本（分页 30/60/120 用）、二进制、>2MB 三态。
const README_LINES = Array.from({ length: 120 }, (_, i) => `第 ${i + 1} 行：AnyFind 预览分页样例`)
const READ_TEXT_FILES = {
  [`${ROOT}/README.md`]: README_LINES,
  [`${ROOT}/notes.txt`]: ['单行文件', '第二行'],
}
const READ_BINARY_FILES = new Set([`${ROOT}/logo.png`, `${ROOT}/data.bin`])
const READ_TOO_LARGE_FILES = new Set([`${ROOT}/huge.txt`])

// format=base64 字节通路夹具（契约补丁：≤2MB 整文件 base64，文本与二进制通用）。
const LOGO_BYTES = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), // PNG magic
  Buffer.alloc(64, 0x7f),
])
const DATA_BIN_BYTES = Buffer.from([0x00, 0x01, 0x02, 0xff, 0x00, 0x7f])

// 最小合法单页 PDF（xref 偏移按实际字节计算，pdfjs 可解析）。
function buildMiniPdf() {
  const stream = 'BT /F1 24 Tf 20 50 Td (AnyFind) Tj ET\n'
  const objects = [
    '1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n',
    '2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n',
    '3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n',
    `4 0 obj\n<< /Length ${stream.length} >>\nstream\n${stream}endstream\nendobj\n`,
    '5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n',
  ]
  let body = '%PDF-1.4\n'
  const offsets = []
  for (const o of objects) {
    offsets.push(body.length)
    body += o
  }
  const xrefStart = body.length
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`
  for (const off of offsets) body += `${String(off).padStart(10, '0')} 00000 n \n`
  body += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xrefStart}\n%%EOF\n`
  return Buffer.from(body, 'latin1')
}

const READ_BASE64_FILES = new Map([
  [`${ROOT}/logo.png`, LOGO_BYTES],
  [`${ROOT}/data.bin`, DATA_BIN_BYTES],
  [`${ROOT}/slide.pdf`, buildMiniPdf()],
  [`${ROOT}/README.md`, Buffer.from(README_LINES.join('\n'), 'utf8')],
  [`${ROOT}/notes.txt`, Buffer.from('单行文件\n第二行', 'utf8')],
])

function isAbsolutePath(p) {
  return /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('\\\\') || p.startsWith('/')
}

function normalizePath(p) {
  const slashed = p.replace(/\\/g, '/')
  const stripped = slashed.replace(/\/+$/, '')
  return stripped === '' ? slashed : stripped
}

// /api/file/op 校验（复刻契约 v0.2 的 4xx 判定；插件代理原样转发，400 由此产生）。
const FILE_OPS = ['rename', 'copy', 'cut', 'recycle', 'newFile', 'newDir']

function validateOpBody(body) {
  if (body === null || typeof body !== 'object' || Array.isArray(body)) return '请求体必须是 JSON 对象'
  if (typeof body.op !== 'string' || !FILE_OPS.includes(body.op)) return `op 不支持: ${String(body.op)}`
  if (!Array.isArray(body.paths)) return 'paths 必须是数组'
  if (body.paths.length === 0) return 'paths 不能为空'
  if (body.paths.length > 128) return `paths 超过上限 128（当前 ${body.paths.length} 条）`
  for (const p of body.paths) {
    if (typeof p !== 'string' || p.trim() === '') return 'paths 必须是非空绝对路径字符串'
  }
  const target = typeof body.target === 'string' ? body.target.trim() : ''
  const newName = typeof body.newName === 'string' ? body.newName.trim() : ''
  if ((body.op === 'copy' || body.op === 'cut') && target === '') return `${body.op} 需要 target 目标目录`
  if ((body.op === 'rename' || body.op === 'newFile' || body.op === 'newDir') && newName === '') {
    return `${body.op} 需要 newName`
  }
  if ((body.op === 'rename' || body.op === 'newFile' || body.op === 'newDir') && body.paths.length !== 1) {
    return `${body.op} 只接受单条 path（当前 ${body.paths.length} 条）`
  }
  if (body.op === 'rename' && target === '') return 'rename 需要 target（所在目录）'
  return null
}

createServer((req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost')
  if (url.pathname === '/api/health') {
    send(res, 200, { ok: true, version: '0.1.0-mock', index: { files: 12034, sessions: 86, skills: 42, memories: 17 }, indexing: false })
    return
  }
  if (url.pathname === '/api/search') {
    const q = url.searchParams.get('q') ?? ''
    const kinds = (url.searchParams.get('kinds') ?? '').split(',').map((k) => k.trim()).filter(Boolean)
    const limit = Number(url.searchParams.get('limit') ?? '20') || 20
    const groups = {}
    for (const [kind, hits] of Object.entries(HITS)) {
      groups[kind] = kinds.length > 0 && !kinds.includes(kind) ? [] : hits.slice(0, limit)
    }
    send(res, 200, { q, took_ms: 7, groups, truncated: { session: true } })
    return
  }
  if (url.pathname === '/api/resume' && req.method === 'POST') {
    let raw = ''
    req.on('data', (c) => { raw += c })
    req.on('end', () => {
      const body = JSON.parse(raw || '{}')
      if (typeof body.agent !== 'string' || typeof body.session_id !== 'string') {
        send(res, 200, { ok: false, error: 'agent 与 session_id 必填' })
        return
      }
      send(res, 200, { ok: true, command: `${body.agent === 'codex' ? 'codex resume' : 'claude --resume'} ${body.session_id}` })
    })
    return
  }
  if (url.pathname.startsWith('/api/session/')) {
    send(res, 200, { session_id: url.pathname.split('/').pop(), agent: 'claude-code', project: 'mock', cwd: 'D:/coding', messages: [{ role: 'user', ts: '2026-10-18T03:12:00', text: 'mock 原文' }] })
    return
  }

  // ---- 文件端点（契约 v0.2 docs/api-contract.md）：list / stat 假数据 ----
  // read / op 不在本 mock 范围（走下方 404 兜底，用于验证代理的非 200 透传）。
  if (url.pathname === '/api/file/drives') {
    // 2026-11-21 v0.2 补充：盘符枚举（无参数，永远 200）。
    // 覆盖契约 type 枚举的四个常见值 + unknown（网络盘归入）。
    send(res, 200, { drives: [
      { path: 'C:/', type: 'fixed' },
      { path: 'D:/', type: 'fixed' },
      { path: 'E:/', type: 'removable' },
      { path: 'F:/', type: 'cdrom' },
      { path: 'Z:/', type: 'unknown' },
    ] })
    return
  }
  if (url.pathname === '/api/file/list') {
    const path = url.searchParams.get('path') ?? ''
    if (path === '') { send(res, 400, { error: 'path 必填' }); return }
    if (!isAbsolutePath(path)) { send(res, 400, { error: 'path 必须是绝对路径' }); return }
    const norm = normalizePath(path)
    if (norm !== ROOT && norm !== ROOT_PARENT) { send(res, 404, { error: `目录不存在或不可读: ${path}` }); return }
    const noise = url.searchParams.get('noise') ?? '0'
    let max = 500
    const maxRaw = url.searchParams.get('max')
    if (maxRaw !== null && Number.isFinite(Number(maxRaw))) max = Math.min(Math.max(Math.trunc(Number(maxRaw)), 1), 2000)
    const kept = ROOT_ENTRIES.filter((e) => noise === '1' || !(e.is_dir && NOISE_DIRS.has(e.name.toLowerCase())))
    const sorted = kept.slice().sort((a, b) => {
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1
      const al = a.name.toLowerCase()
      const bl = b.name.toLowerCase()
      if (al !== bl) return al < bl ? -1 : 1
      return a.name < b.name ? -1 : 1
    })
    const truncated = sorted.length > max
    send(res, 200, { path, entries: sorted.slice(0, max), truncated })
    return
  }
  if (url.pathname === '/api/file/stat') {
    const path = url.searchParams.get('path') ?? ''
    if (path === '') { send(res, 400, { error: 'path 必填' }); return }
    if (!isAbsolutePath(path)) { send(res, 400, { error: 'path 必须是绝对路径' }); return }
    const norm = normalizePath(path)
    if (norm === ROOT || norm === ROOT_PARENT) {
      send(res, 200, {
        name: norm.slice(norm.lastIndexOf('/') + 1),
        is_dir: true, size: 0, mtime: DIR_MTIME,
      })
      return
    }
    const file = KNOWN_FILES[norm]
    if (file !== undefined) { send(res, 200, file); return }
    send(res, 404, { error: `不存在: ${path}` })
    return
  }
  if (url.pathname === '/api/file/read') {
    const path = url.searchParams.get('path') ?? ''
    if (path === '') { send(res, 400, { error: 'path 必填' }); return }
    if (!isAbsolutePath(path)) { send(res, 400, { error: 'path 必须是绝对路径' }); return }
    const norm = normalizePath(path)
    const format = url.searchParams.get('format')
    if (format !== null && format !== 'base64') { send(res, 400, { error: `format 仅支持 base64: ${format}` }); return }
    const textLines = READ_TEXT_FILES[norm]
    const isBinary = READ_BINARY_FILES.has(norm)
    const tooLarge = READ_TOO_LARGE_FILES.has(norm)
    const b64Bytes = READ_BASE64_FILES.get(norm)
    if (textLines === undefined && !isBinary && !tooLarge && b64Bytes === undefined) {
      send(res, 404, { error: `不存在: ${path}` })
      return
    }
    // format=base64 字节通路：≤2MB 整文件 base64（文本与二进制通用，binary 标志随行带回）；
    // >2MB → 413；不传 format 时本分支不生效（行为与旧契约完全一致）。
    // 契约响应仅 {content_base64, binary, truncated}——不回 size（内核同）。
    if (format === 'base64') {
      if (tooLarge) { send(res, 413, { error: '文件超过 2MB，无法读取' }); return }
      if (b64Bytes === undefined) { send(res, 404, { error: `不存在: ${path}` }); return }
      send(res, 200, { binary: isBinary, content_base64: b64Bytes.toString('base64'), truncated: false })
      return
    }
    // 二进制判定先于尺寸判定（契约）：前 8KB 含 NUL → { binary: true }（无 content）
    if (isBinary) { send(res, 200, { binary: true }); return }
    if (tooLarge) { send(res, 413, { error: '文件超过 2MB，无法读取' }); return }
    let offset = Number(url.searchParams.get('offset') ?? '0')
    if (!Number.isFinite(offset) || offset < 0) offset = 0
    offset = Math.trunc(offset)
    let requested = Number(url.searchParams.get('lines') ?? '30')
    if (!Number.isFinite(requested)) requested = 30
    requested = Math.min(Math.max(Math.trunc(requested), 1), 500)
    const window = textLines.slice(offset, offset + requested)
    send(res, 200, {
      content: window.join('\n'),
      totalLines: textLines.length,
      offset,
      lines: window.length,
      truncated: offset + window.length >= textLines.length,
      binary: false,
    })
    return
  }
  if (url.pathname === '/api/file/op') {
    if (req.method !== 'POST') { send(res, 405, { error: 'method not allowed' }); return }
    let raw = ''
    req.on('data', (c) => { raw += c })
    req.on('end', () => {
      let body
      try {
        body = JSON.parse(raw === '' ? '{}' : raw)
      } catch {
        send(res, 400, { error: '请求体非合法 JSON' })
        return
      }
      const invalid = validateOpBody(body)
      if (invalid !== null) { send(res, 400, { error: invalid }); return }
      // 执行模拟两态：路径含 'locked' → 该项失败（单项失败不改 HTTP 状态），其余成功。
      const results = body.paths.map((p) => (String(p).includes('locked')
        ? { path: p, ok: false, error: `拒绝访问: ${p}` }
        : { path: p, ok: true }))
      send(res, 200, { ok: true, results })
    })
    return
  }

  send(res, 404, { error: 'not found' })
}).listen(port, '127.0.0.1', () => {
  console.log(`mock-core listening on http://127.0.0.1:${port}`)
})
