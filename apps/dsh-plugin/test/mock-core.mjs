// mock-core.mjs — 按 docs/api-contract.md v0.1 返回假数据的统一搜索内核 mock。
// 用法：node test/mock-core.mjs [port]（默认 18765，避免与真内核 8765 撞车）
import { createServer } from 'node:http'

const port = Number(process.argv[2] ?? '18765')

const HITS = {
  file: [
    { kind: 'file', title: 'nginx.conf', path: 'D:/coding/filefind/nginx.conf', snippet: null, mtime: '2026-10-12T08:30:00', agent: null, project: 'filefind', session_id: null, cwd: null, score: 0.91 },
  ],
  content: [
    { kind: 'content', title: '部署笔记.md', path: 'D:/notes/deploy.md', snippet: '反代配置 <mark>nginx</mark> 的 server_name 要带上内网域名', mtime: '2026-10-10T14:02:00', agent: null, project: null, session_id: null, cwd: null, score: 0.77 },
  ],
  session: [
    { kind: 'session', title: '给 dsh 插件加搜索面板', path: null, snippet: '用户：之前 <mark>nginx</mark> 反代怎么配的来着？\n助手：上次的配置在…', mtime: '2026-10-18T03:12:00', agent: 'claude-code', project: 'dsh-plugin', session_id: 'sess-abc-123', cwd: 'D:/coding/filefind/dsh-plugin', score: 0.83 },
    { kind: 'session', title: '内核索引联调', path: null, snippet: '<mark>nginx</mark> access log 解析', mtime: '2026-10-15T09:40:00', agent: 'codex', project: 'filefind', session_id: 'sess-def-456', cwd: 'D:/coding/filefind', score: 0.66 },
  ],
  skill: [
    { kind: 'skill', title: 'opencli-adapter-author', path: 'C:/Users/x/.agents/skills/opencli-adapter-author/SKILL.md', snippet: '为 OpenCLI 编写新站点 <mark>nginx</mark> 适配器…', mtime: '2026-09-30T11:00:00', agent: 'claude-code', project: null, session_id: null, cwd: null, score: 0.52 },
  ],
  memory: [
    { kind: 'memory', title: 'CLAUDE.md', path: 'D:/coding/filefind/CLAUDE.md', snippet: '本项目反代统一走 <mark>nginx</mark>，不用 caddy', mtime: '2026-10-01T02:00:00', agent: 'claude-code', project: 'filefind', session_id: null, cwd: null, score: 0.71 },
  ],
}

function send(res, code, body) {
  res.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'access-control-allow-origin': '*' })
  res.end(JSON.stringify(body))
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
  send(res, 404, { error: 'not found' })
}).listen(port, '127.0.0.1', () => {
  console.log(`mock-core listening on http://127.0.0.1:${port}`)
})
