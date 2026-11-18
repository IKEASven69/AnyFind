#!/usr/bin/env node
/**
 * anyfindd `mcp` 子命令端到端冒烟（零依赖，node 直跑）
 *
 *   cd D:/coding/anyfind && node scripts/mcp-smoke.mjs     ← 在 anyfind 仓库根下跑
 *   node D:/coding/anyfind/scripts/mcp-smoke.mjs           ← 任意目录均可（绝对路径）
 *
 * 注意：本脚本在 anyfind 仓库内，node 按当前目录解析相对模块路径——
 * 在仓库外（如工作区根 D:\coding）执行 `node scripts/mcp-smoke.mjs` 会
 * MODULE_NOT_FOUND；请用上面两种形式之一（脚本内部路径一律按本文件定位，与 cwd 无关）。
 * 前置：需要 core/target/release/anyfindd.exe（缺失时先 `cargo build --release -p anyfindd`）。
 *
 * 做什么：spawn core/target/release/anyfindd.exe mcp --no-index --db <%TEMP%>/anyfind-mcp-smoke/index.db
 * （临时空库，不碰真实索引库），stdin/stdout 按换行分隔的 JSON-RPC 2.0 逐行收发。
 *
 * 断言（16 项）：
 *   1-3   initialize：protocolVersion 回显请求值、serverInfo{name,version}、capabilities.tools
 *   4     notifications/initialized：无输出（300ms 零行）且进程不崩
 *   5-6   tools/list：恰 3 个工具、名字与顺序正确
 *   7-8   tools/call anyfind_search q=nginx：无 error、isError=false；命中文本可解析为
 *         JSON 数组且元素带 string kind（空库空数组也算过，形态必须对）
 *   9-10  anyfind_file_read README.md：isError=false / content 为字符串 / totalLines>=1；
 *         content 以 "# AnyFind" 开头
 *   11-12 anyfind_file_stat README.md：name=README.md；is_dir=false / size>0 / mtime 字符串
 *   13    ping -> {}
 *   14    未知方法 -> -32601
 *   15    stdout 纯净度：响应行数恰等于带 id 请求数，且全部可解析（无协议外输出）
 *   16    stdin EOF 后子进程退出码 0
 *
 * 退出码：0 = 全部通过；1 = 存在失败。临时库目录用完即删；只读真实文件（README.md）。
 */
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const EXE = join(ROOT, "core", "target", "release", "anyfindd.exe");
const TMP = join(process.env.TEMP || tmpdir(), "anyfind-mcp-smoke");
const DB = join(TMP, "index.db");
const README = "D:/coding/anyfind/README.md";
const RPC_TIMEOUT_MS = 30_000;

let passed = 0, failed = 0;
const fails = [];
function ok(msg) { passed++; console.log(`  [ok]   ${msg}`); }
function fail(msg, extra) {
  failed++; fails.push(msg);
  console.log(`  [FAIL] ${msg}${extra !== undefined ? ` — ${JSON.stringify(extra)}` : ""}`);
}

console.log("anyfindd mcp 端到端冒烟（stdio JSON-RPC 2.0，临时空库）");
console.log(`目标: ${EXE}`);
console.log(`临时库: ${DB}\n`);

// 前置门禁：release 二进制缺失时给出可执行提示（首跑常见），而不是裸 ENOENT。
if (!existsSync(EXE)) {
  fail(`找不到 ${EXE}；先构建：cd core && cargo build --release -p anyfindd`);
  rmSync(TMP, { recursive: true, force: true });
  console.log(`\n结果: 失败（0 通过 / ${failed} 失败）`);
  process.exit(1);
}

// 每次运行都从全新空库开始（含 WAL/shm/names.bin 一并清掉）。
rmSync(TMP, { recursive: true, force: true });
mkdirSync(TMP, { recursive: true });

const child = spawn(EXE, ["mcp", "--no-index", "--db", DB], { stdio: ["pipe", "pipe", "pipe"] });
let stderrText = "";
child.stderr.setEncoding("utf8");
child.stderr.on("data", (c) => { stderrText += c; });
const exited = new Promise((resolve) => child.once("exit", (code) => resolve(code)));
const spawnError = new Promise((_, reject) => child.once("error", reject));

// ---- 行协议：stdout 按 \n 切分，按 id 匹配挂起请求；任何协议外输出都记账 ----
const pending = new Map(); // String(id) -> resolve
let linesReceived = 0, linesUnparsable = 0, idRequests = 0;
let buf = "";
child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
  buf += chunk;
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl).trim();
    buf = buf.slice(nl + 1);
    if (!line) continue;
    linesReceived++;
    let msg;
    try { msg = JSON.parse(line); } catch { linesUnparsable++; continue; }
    const waiter = pending.get(String(msg.id));
    pending.delete(String(msg.id));
    if (waiter) waiter(msg);
  }
});

function rpc(id, method, params) {
  idRequests++;
  const req = { jsonrpc: "2.0", id, method };
  if (params !== undefined) req.params = params;
  const future = new Promise((resolve, reject) => {
    pending.set(String(id), resolve);
    const t = setTimeout(() => {
      pending.delete(String(id));
      reject(new Error(`请求超时（${RPC_TIMEOUT_MS}ms）: ${method} id=${id}`));
    }, RPC_TIMEOUT_MS);
    pending.set(String(id), (m) => { clearTimeout(t); resolve(m); });
  });
  child.stdin.write(JSON.stringify(req) + "\n");
  return future;
}
function notify(method) {
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method }) + "\n");
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  // 1-3 initialize
  const init = await rpc(1, "initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "mcp-smoke", version: "0" },
  });
  if (init?.result?.protocolVersion === "2025-06-18") ok('initialize 回显 protocolVersion="2025-06-18"');
  else fail("initialize 回显 protocolVersion", init?.result?.protocolVersion);
  const si = init?.result?.serverInfo;
  if (si?.name === "anyfind" && typeof si?.version === "string" && si.version.length > 0)
    ok(`initialize.serverInfo {name:"anyfind", version:"${si.version}"}`);
  else fail("initialize.serverInfo", si);
  if (init?.result?.capabilities?.tools !== undefined && init?.result?.capabilities?.tools !== null
    && typeof init?.result?.capabilities?.tools === "object")
    ok("initialize.capabilities.tools 存在");
  else fail("initialize.capabilities.tools", init?.result?.capabilities);

  // 4 notifications/initialized：无输出、不崩
  const before = linesReceived;
  notify("notifications/initialized");
  await sleep(300);
  if (linesReceived === before && child.exitCode === null && !child.killed)
    ok("notifications/initialized 无输出且进程存活（300ms 零行）");
  else
    fail("notifications/initialized 应无输出且不崩", { linesReceived, exitCode: child.exitCode });

  // 13 ping（紧跟 notification 之后：下一行响应必须属于本请求）
  const ping = await rpc(2, "ping");
  if (ping?.result && Object.keys(ping.result).length === 0 && !("error" in ping))
    ok("ping -> {}");
  else fail("ping -> {}", ping);

  // 5-6 tools/list
  const list = await rpc(3, "tools/list");
  const tools = list?.result?.tools;
  if (Array.isArray(tools) && tools.length === 3) ok("tools/list 恰 3 个工具");
  else fail("tools/list 恰 3 个工具", tools?.length);
  const names = (tools || []).map((t) => t?.name);
  const want = ["anyfind_search", "anyfind_file_read", "anyfind_file_stat"];
  if (JSON.stringify(names) === JSON.stringify(want)) ok(`工具名与顺序正确: ${names.join(", ")}`);
  else fail("工具名与顺序", names);

  // 7-8 anyfind_search（临时空库：空命中也算过，形态必须对）
  const search = await rpc(4, "tools/call", {
    name: "anyfind_search",
    arguments: { q: "nginx" },
  });
  if (!("error" in search) && search?.result?.isError === false)
    ok("anyfind_search q=nginx：无 error、isError=false");
  else fail("anyfind_search q=nginx 应为成功结果", search?.error ?? search?.result);
  let hits = null;
  try { hits = JSON.parse(search?.result?.content?.[0]?.text); } catch { hits = null; }
  if (Array.isArray(hits) && hits.every((h) => typeof h?.kind === "string"))
    ok(`anyfind_search 命中文本为 JSON 数组且元素带 kind（${hits.length} 命中，空库 0 也算过）`);
  else fail("anyfind_search 命中文本形态", search?.result?.content?.[0]?.text);

  // 9-10 anyfind_file_read README.md（真实只读文件）
  const read = await rpc(5, "tools/call", {
    name: "anyfind_file_read",
    arguments: { path: README },
  });
  let readPayload = null;
  try { readPayload = JSON.parse(read?.result?.content?.[0]?.text); } catch { readPayload = null; }
  if (!("error" in read) && read?.result?.isError === false
    && typeof readPayload?.content === "string" && readPayload?.totalLines >= 1)
    ok(`anyfind_file_read README.md：isError=false、content 字符串、totalLines=${readPayload?.totalLines}`);
  else fail("anyfind_file_read README.md", read?.error ?? read?.result?.content?.[0]?.text);
  if (typeof readPayload?.content === "string" && readPayload.content.startsWith("# AnyFind"))
    ok('anyfind_file_read content 以 "# AnyFind" 开头');
  else fail('anyfind_file_read content 开头', readPayload?.content?.slice(0, 40));

  // 11-12 anyfind_file_stat README.md
  const stat = await rpc(6, "tools/call", {
    name: "anyfind_file_stat",
    arguments: { path: README },
  });
  let statPayload = null;
  try { statPayload = JSON.parse(stat?.result?.content?.[0]?.text); } catch { statPayload = null; }
  if (!("error" in stat) && stat?.result?.isError === false && statPayload?.name === "README.md")
    ok("anyfind_file_stat：isError=false 且 name=README.md");
  else fail("anyfind_file_stat name=README.md", stat?.error ?? stat?.result?.content?.[0]?.text);
  if (statPayload?.is_dir === false && statPayload?.size > 0 && typeof statPayload?.mtime === "string")
    ok(`anyfind_file_stat：is_dir=false、size=${statPayload?.size}、mtime="${statPayload?.mtime}"`);
  else fail("anyfind_file_stat 元信息", statPayload);

  // 14 未知方法
  const bogus = await rpc(7, "bogus/method");
  if (bogus?.error?.code === -32601 && bogus?.id === 7) ok("未知方法 -> -32601");
  else fail("未知方法 -> -32601", bogus?.error);

  // 16 stdin EOF → 子进程退出码
  child.stdin.end();
  const code = await exited;
  if (code === 0) ok("stdin EOF 后子进程退出码 0");
  else fail("子进程退出码", code);

  // 15 stdout 纯净度（最后核对：响应行数恰等于带 id 请求数，全部可解析）
  if (linesReceived === idRequests && linesUnparsable === 0)
    ok(`stdout 纯净：${linesReceived} 行响应 === ${idRequests} 个带 id 请求，全部可解析`);
  else fail("stdout 纯净度", { linesReceived, idRequests, linesUnparsable });
}

try {
  await Promise.race([main(), spawnError]);
} catch (e) {
  fail("冒烟执行异常", e?.message ?? String(e));
  child.kill();
}

if (stderrText.trim()) console.log(`\n(stderr 日志，应只含横幅/诊断，非协议):\n${stderrText.trim().split("\n").map((l) => `  ${l}`).join("\n")}`);
rmSync(TMP, { recursive: true, force: true });
console.log(`\n结果: ${failed === 0 ? "全部通过" : "失败"}（${passed} 通过 / ${failed} 失败）`);
if (failed > 0) { console.log(fails.map((f) => `  - ${f}`).join("\n")); process.exitCode = 1; }
