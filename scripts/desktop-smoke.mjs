#!/usr/bin/env node
/**
 * AnyFind 桌面端冒烟门禁（零依赖，node 直跑）
 *
 *   node scripts/desktop-smoke.mjs        （从仓库任意位置均可，路径按本文件定位）
 *
 * 覆盖面（对应 v0.2 文件工作台功能面）：
 *   1. 读取 apps/desktop/ui/index.html，正则提取全部内联 <script> 块；
 *   2. 每块用 node:vm 仅编译不执行做语法门禁，语法错报 index.html 行号；
 *   3. 结构锚点断言：文件端点封装 / 预览通路（文本分页·图片·PDF·富格式·媒体播放）/
 *      目录树 / 盘符首页 / 收藏夹（anyfind.favorites）/ 多标签 / 右键写操作 / 内核错误文案透出；
 *   4. 本地资源存在性：vendor/pdfjs 主包与 worker、cmaps/、standard_fonts/
 *      抽查 3 个文件 + 外部模块 vendor/preview/rich.js（可编译时顺带语法门禁）。
 *
 * 退出码：0 = 全部通过；1 = 存在失败。不改 ui/index.html，只读。
 */
import { readFileSync, existsSync, statSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const UI_DIR = join(ROOT, "apps", "desktop", "ui");
const HTML_PATH = join(UI_DIR, "index.html");
const REL = p => relative(ROOT, p).replace(/\\/g, "/");

let passed = 0, failed = 0;
const fails = [];
function ok(msg) { passed++; console.log(`  [ok]   ${msg}`); }
function fail(msg) { failed++; fails.push(msg); console.log(`  [FAIL] ${msg}`); }
function lineOf(html, index) { return html.slice(0, index).split("\n").length; }
/** 从 SyntaxError 里取源码行号：优先 e.lineNumber，缺则解析 stack 的 `filename:line[:col]`（Node 24 vm 不设 lineNumber） */
function errLine(e, filename) {
  if (typeof e.lineNumber === "number" && e.lineNumber > 0) return e.lineNumber;
  const escFn = filename.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  for (const line of (e.stack || "").split("\n").slice(0, 5)) {
    let m = line.match(new RegExp(escFn + ":(\\d+)(?::\\d+)?"));
    if (m) return Number(m[1]);
    m = line.match(/\bat\b[^\n]*?:(\d+):\d+/);
    if (m) return Number(m[1]);
  }
  return null;
}

console.log("AnyFind 桌面端冒烟门禁（vanilla JS 单文件，无构建步）");
console.log(`目标: ${REL(HTML_PATH)}\n`);

/* ── 1. 读取 + 提取内联 <script> 块 ─────────────────────────── */
let html = "";
try {
  html = readFileSync(HTML_PATH, "utf8");
  ok(`读取 index.html（${html.split("\n").length} 行，${html.length} 字符）`);
} catch (e) {
  fail(`读取 index.html 失败: ${e.message}`);
  console.log(`\n结果: 失败（${failed} 项）`);
  process.exitCode = 1;
  throw e;
}

const blocks = [];   // { code, htmlLine, attrs }
const externals = []; // { src, htmlLine }
const scriptRe = /<script\b([^>]*)>([\s\S]*?)<\/script>/gi;
let m;
while ((m = scriptRe.exec(html)) !== null) {
  const attrs = m[1] || "";
  const codeStart = m.index + m[0].indexOf(m[2]);
  const src = /src\s*=\s*["']([^"']+)["']/i.exec(attrs);
  if (src) externals.push({ src: src[1], htmlLine: lineOf(html, m.index) });
  else if (m[2].trim()) blocks.push({ code: m[2], htmlLine: lineOf(html, codeStart) });
}
console.log(`── 1) 内联 <script> 块提取 ──`);
blocks.length > 0
  ? ok(`提取到 ${blocks.length} 个内联块（外部引用 ${externals.length} 个另行检查）: ${blocks.map(b => `index.html:${b.htmlLine}`).join(", ")}`)
  : fail("未提取到任何内联 <script> 块（文件结构变更或提取正则失效）");
const js = blocks.map(b => b.code).join("\n;\n");

/* ── 2) 语法门禁：node:vm 仅编译不执行 ─────────────────────── */
console.log(`── 2) 语法门禁（node:vm 编译，不执行页面代码）──`);
for (const [i, b] of blocks.entries()) {
  const label = `内联块 #${i + 1}（始于 index.html:${b.htmlLine}，${b.code.split("\n").length} 行）`;
  try {
    new vm.Script(b.code, { filename: `index.html#inline${i + 1}` });
    ok(`${label} 编译通过`);
  } catch (e) {
    if (e instanceof SyntaxError) {
      const src = errLine(e, `index.html#inline${i + 1}`);
      const at = src == null ? "" : ` → index.html:${b.htmlLine + src - 1}`;
      fail(`${label} 语法错误${at}: ${e.message}`);
    } else {
      fail(`${label} 编译异常: ${e.message}`);
    }
  }
}

/* ── 3) 结构锚点断言（v0.2 功能面）─────────────────────────── */
console.log(`── 3) 结构锚点断言 ──`);
/** @param {[string, RegExp, "js"|"html", string][]} anchors */
function assertAnchors(anchors) {
  for (const [name, re, scope, why] of anchors) {
    const hay = scope === "js" ? js : html;
    const hit = re.exec(hay);
    if (!hit) { fail(`${scope === "js" ? "JS" : "DOM"} 锚点 ${name}（${why}）未命中: ${re}`); continue; }
    // 锚点直接在原文件里定位首次出现，给可点击行号（内联块本就是 html 一部分）
    const inHtml = re.exec(html);
    const at = inHtml ? `index.html:${lineOf(html, inHtml.index)}` : "内联 JS 内";
    ok(`${scope === "js" ? "JS" : "DOM"} 锚点 ${name}（${why}）@ ${at}`);
  }
}
assertAnchors([
  // —— 内核文件端点封装（契约 v0.2 /api/file/{list,read,stat,op}）——
  ["apiFileList",   /(?:async\s+)?function\s+apiFileList\s*\(/,    "js", "目录懒加载列举"],
  ["apiFileRead",   /(?:async\s+)?function\s+apiFileRead\s*\(/,    "js", "文本分页读取"],
  ["apiFileBase64", /(?:async\s+)?function\s+apiFileBase64\s*\(/,  "js", "base64 取字节（图片/PDF 预览通路）"],
  ["apiFileStat",   /(?:async\s+)?function\s+apiFileStat\s*\(/,    "js", "文件元信息"],
  ["apiFileOp",     /(?:async\s+)?function\s+apiFileOp\s*\(/,      "js", "六种写操作端点"],
  // —— 预览通路 ——
  ["loadImage",     /(?:async\s+)?function\s+loadImage\s*\(/,      "js", "图片 base64→data URL 预览"],
  ["ensurePdf",     /(?:async\s+)?function\s+ensurePdf\s*\(/,      "js", "PDF 本地 pdfjs 预览"],
  ["destroyRich",   /function\s+destroyRich\s*\(/,                 "js", "富预览句柄释放（raf/监听器）"],
  ["TEXT_PAGE_SIZES", /TEXT_PAGE_SIZES\s*=\s*\[\s*30,\s*60,\s*120\s*\]/, "js", "文本分页 30/60/120 行"],
  ["pdfjs 主包引用", /vendor\/pdfjs\/pdf\.min\.js/,               "js", "PDF 预览运行时加载"],
  ["pdfjs worker 引用", /vendor\/pdfjs\/pdf\.worker\.min\.js/,     "js", "PDF worker 配置"],
  ["pdfjs cmaps 引用", /vendor\/pdfjs\/cmaps\//,                   "js", "CJK 字符映射"],
  ["pdfjs 字体引用", /vendor\/pdfjs\/standard_fonts\//,            "js", "标准字体回退"],
  // —— 目录树 / 多标签 ——
  ["renderTree",    /function\s+renderTree\s*\(/,                  "js", "目录树渲染"],
  ["closeTab",      /function\s+closeTab\s*\(/,                    "js", "多标签关闭"],
  // —— 盘符首页（契约 v0.2 GET /api/file/drives）——
  ["apiFileDrives",     /(?:async\s+)?function\s+apiFileDrives\s*\(/,     "js", "盘符枚举端点封装"],
  ["DRIVE_ICONS",       /DRIVE_ICONS\s*=\s*\{/,                           "js", "盘符 type 图标区分（fixed/removable/cdrom/ram/unknown）"],
  ["renderDrivesHome",  /function\s+renderDrivesHome\s*\(/,               "js", "树根级盘符首页（点击进各盘根）"],
  // —— 收藏夹（localStorage anyfind.favorites）——
  ["fav storage key", /["']anyfind\.favorites["']/,               "js", "收藏夹 localStorage 持久键"],
  ["toggleFav",       /function\s+toggleFav\s*\(/,                "js", "树/结果行星标切换收藏"],
  ["renderFavBar",    /function\s+renderFavBar\s*\(/,             "js", "收藏栏渲染（拖拽排序）"],
  ["fav-bar DOM",     /id\s*=\s*["']fav-bar["']/,                 "html", "收藏栏容器"],
  // —— 媒体播放（base64→Blob URL，>2MB 降级）——
  ["MEDIA_MIME",      /MEDIA_MIME\s*=\s*\{/,                      "js", "音视频扩展名→MIME 判定"],
  ["mediaKindFor",    /function\s+mediaKindFor\s*\(/,             "js", "音视频类型判定（video/audio）"],
  ["loadMedia",       /(?:async\s+)?function\s+loadMedia\s*\(/,   "js", "媒体 base64→Blob URL 加载"],
  ["video 控件",      /document\.createElement\(\s*["']video["']\s*\)/, "js", "预览面板 <video> 控件"],
  ["audio 控件",      /document\.createElement\(\s*["']audio["']\s*\)/, "js", "预览面板 <audio> 控件"],
  ["MEDIA_LIMIT",     /MEDIA_LIMIT\s*=\s*2\s*\*\s*1024\s*\*\s*1024/,    "js", ">2MB 媒体降级阈值（对齐内核 READ_SIZE_MAX）"],
  // —— 右键写操作菜单 ——
  ["runFileOp",     /(?:async\s+)?function\s+runFileOp\s*\(/,      "js", "写操作执行"],
  ["newEntryFlow",  /(?:async\s+)?function\s+newEntryFlow\s*\(/,   "js", "新建文件/文件夹流程"],
  ["op:recycle",    /\brecycle\b/,                                  "js", "删除→回收站（非永久删除）"],
  ["op:newFile",    /\bnewFile\b/,                                  "js", "新建文件操作字"],
  ["op:newDir",     /\bnewDir\b/,                                   "js", "新建文件夹操作字"],
  // —— 内核错误文案透出 ——
  ["kernelResp",    /(?:async\s+)?function\s+kernelResp\s*\(/,     "js", "4xx/5xx 读 {error} 体而非裸状态码"],
  // —— DOM 节点 ——
  ["#preview",      /id\s*=\s*["']preview["']/,                    "html", "预览面板容器"],
  ["#tree",         /id\s*=\s*["']tree["']/,                       "html", "目录树容器"],
  ["#tabs",         /id\s*=\s*["']tabs["']/,                       "html", "多标签栏容器"],
]);

/* ── 4) 引用的本地资源存在性 ────────────────────────────────── */
console.log(`── 4) 本地资源存在性（相对 ${REL(UI_DIR)}）──`);
const need = [
  ["vendor/pdfjs/pdf.min.js", "file"],
  ["vendor/pdfjs/pdf.worker.min.js", "file"],
  ["vendor/pdfjs/cmaps", "dir"],
  ["vendor/pdfjs/standard_fonts", "dir"],
  // 抽查 3 个文件：cmaps 1 个 + standard_fonts 1 个 + 外部模块 rich.js 1 个
  ["vendor/pdfjs/cmaps/B5-H.bcmap", "file"],
  ["vendor/pdfjs/standard_fonts/FoxitFixed.pfb", "file"],
  ["vendor/preview/rich.js", "file"],
];
for (const [rel, kind] of need) {
  const p = join(UI_DIR, ...rel.split("/"));
  if (!existsSync(p)) { fail(`${kind === "dir" ? "目录" : "文件"} ${rel} 不存在`); continue; }
  if (kind === "dir") {
    const n = readdirSync(p).length;
    n > 0 ? ok(`目录 ${rel}/ 存在（${n} 项）`) : fail(`目录 ${rel}/ 为空`);
  } else {
    const size = statSync(p).size;
    size > 0 ? ok(`文件 ${rel} 存在（${size} B）`) : fail(`文件 ${rel} 为空文件`);
  }
}
// cmaps 目录内容类型抽查（pdfjs 按 cMapUrl+文件名 按需拉取 .bcmap）
const cmapDir = join(UI_DIR, "vendor/pdfjs/cmaps");
if (existsSync(cmapDir)) {
  const bcmaps = readdirSync(cmapDir).filter(f => f.endsWith(".bcmap"));
  bcmaps.length > 0 ? ok(`cmaps/ 含 ${bcmaps.length} 个 .bcmap（按需加载格式正确）`) : fail(`cmaps/ 无任何 .bcmap 文件`);
}

// 外部 <script src> 引用：本地路径必须存在（http(s)/协议外的不检查）
for (const ext of externals) {
  if (/^(https?:)?\/\//i.test(ext.src) || /^[a-z]+:/i.test(ext.src)) { ok(`外部引用 ${ext.src} 为远端/协议地址，跳过存在性（index.html:${ext.htmlLine}）`); continue; }
  const p = join(UI_DIR, ...ext.src.split(/[\/\\]/));
  if (!existsSync(p)) { fail(`<script src> 引用的本地文件缺失: ${ext.src}（index.html:${ext.htmlLine}）`); continue; }
  ok(`<script src> 引用 ${ext.src} 存在（index.html:${ext.htmlLine}）`);
  // ES module 语法门禁（best-effort：vm.SourceTextModule 需 --experimental-vm-modules，不可用则降级跳过）
  if (typeof vm.SourceTextModule === "function") {
    try {
      new vm.SourceTextModule(readFileSync(p, "utf8"), { identifier: ext.src });
      ok(`模块语法门禁 ${ext.src} 编译通过（vm.SourceTextModule）`);
    } catch (e) {
      const src = errLine(e, ext.src);
      fail(`模块语法门禁 ${ext.src}: ${e.message}${src != null ? `（${ext.src} 第 ${src} 行）` : ""}`);
    }
  } else {
    console.log(`  [skip] 模块语法门禁 ${ext.src}：vm.SourceTextModule 需 --experimental-vm-modules，本次跳过`);
  }
}

/* ── 汇总 ──────────────────────────────────────────────────── */
console.log("─".repeat(56));
if (failed === 0) {
  console.log(`结果: 通过（${passed} 项检查全部 ok，0 失败）`);
} else {
  console.log(`结果: 失败（通过 ${passed}，失败 ${failed}）：`);
  for (const f of fails) console.log(`  ✗ ${f}`);
}
process.exitCode = failed === 0 ? 0 : 1;
