// rich.js — AnyFind 桌面端富格式预览接入层（本仓库自有代码，MIT，Copyright (c) 2026 IKEASven69）。
// 解析与渲染复用 vendor 内上游解析层（archive/formats/model，MIT，见 VENDORED.md）：
//   - 3D（stl/obj/ply/gltf/glb）→ parseModel + createModelView（Canvas2D，自带拖拽/缩放/降级）
//   - 压缩包（zip/tar/gz/bz2/xz/zst/7z/rar…）→ listArchive 条目列表 + 文本条目点开
//   - TOML / Intel HEX / JCAMP-DX / VCD / KiCad / DFU → 解析后渲染结构化摘要
//     （摘要级弱于 dsh 插件侧 React 正文，见 VENDORED.md 说明）。
// 取字节：内核 /api/file/read?format=base64（≤2MB）→ Tauri convertFileSrc 后备（大文件）。

import { listArchive, readEntryText, formatBytes } from './archive/index.js';
import { parseToml } from './formats/toml.js';
import { parseIntelHex, formatHexDump } from './formats/ihex.js';
import { parseJcamp } from './formats/jcamp.js';
import { parseVcd } from './formats/vcd.js';
import { parseKicad } from './formats/kicad.js';
import { parseDfu } from './formats/dfu.js';
import { parseModel } from './model/detect3d.js';
import { createModelView } from './model/renderer.js';

const MODEL_EXTS = new Set(['stl', 'obj', 'ply', 'gltf', 'glb']);
const ARCHIVE_EXTS = new Set(['zip', 'jar', 'war', 'apk', 'tar', 'gz', 'tgz', 'bz2', 'xz', 'zst', '7z', 'rar']);
const FORMAT_EXTS = new Set(['toml', 'hex', 'jdx', 'dx', 'vcd', 'kicad_sch', 'kicad_pcb', 'dfu']);

/** 扩展名 → 富预览 id（'model' | 'archive' | 'toml' | 'ihex' | 'jcamp' | 'vcd' | 'kicad' | 'dfu' | null）。 */
function extKind(ext) {
  if (MODEL_EXTS.has(ext)) return 'model';
  if (ARCHIVE_EXTS.has(ext)) return 'archive';
  if (ext === 'toml') return 'toml';
  if (ext === 'hex') return 'ihex';
  if (ext === 'jdx' || ext === 'dx') return 'jcamp';
  if (ext === 'vcd') return 'vcd';
  if (ext === 'kicad_sch' || ext === 'kicad_pcb') return 'kicad';
  if (ext === 'dfu') return 'dfu';
  return null;
}

function extOf(path) {
  const i = path.lastIndexOf('.');
  return i <= 0 ? '' : path.slice(i + 1).toLowerCase();
}

async function loadBytes(path) {
  // 首选内核 format=base64（≤2MB）；>2MB 413 → Tauri convertFileSrc 直读磁盘。
  try {
    const d = await window.apiFileBase64(path);
    if (d && d.content_base64) {
      const bin = atob(d.content_base64);
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
      return out;
    }
  } catch (e) { /* 落后备 */ }
  const T = window.T;
  if (T && T.core && typeof T.core.convertFileSrc === 'function') {
    const r = await fetch(T.core.convertFileSrc(path));
    if (r.ok) return new Uint8Array(await r.arrayBuffer());
  }
  throw new Error('无法获取文件字节（>2MB 且 Tauri 直读不可用）');
}

/* ---------- 小 DOM 工具 ---------- */
function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') node.className = v;
    else if (k === 'text') node.textContent = v;
    else if (k.startsWith('on') && typeof v === 'function') node.addEventListener(k.slice(2), v);
    else node.setAttribute(k, v);
  }
  for (const c of children) if (c != null) node.append(c);
  return node;
}
function kv(label, value) {
  if (value === null || value === undefined || value === '') return null;
  return el('div', { class: 'rf-kv' },
    el('span', { class: 'rf-k', text: label }),
    el('span', { class: 'rf-v', text: String(value) }));
}
function kvBox(title, ...pairs) {
  const box = el('div', { class: 'rf-box' }, el('div', { class: 'rf-box-t', text: title }));
  for (const p of pairs) if (p) box.append(p);
  return box;
}

/* ---------- CSS（rf- 前缀，避免与宿主样式冲突） ---------- */
const CSS = `
.rf-root{display:flex;flex-direction:column;gap:10px;height:100%;min-height:0;color:rgba(255,255,255,.88);
  font:12px/1.5 ui-sans-serif,system-ui,"Segoe UI","Microsoft YaHei",sans-serif}
.rf-scroll{overflow:auto;min-height:0;flex:1}
.rf-box{border:1px solid rgba(255,255,255,.12);border-radius:8px;padding:8px 10px;background:rgba(255,255,255,.03)}
.rf-box-t{font-weight:700;letter-spacing:.4px;color:rgba(255,255,255,.45);font-size:10px;text-transform:uppercase;margin-bottom:6px}
.rf-kv{display:flex;gap:8px;padding:1.5px 0}
.rf-k{color:rgba(255,255,255,.45);flex:0 0 110px}
.rf-v{font-family:ui-monospace,Consolas,monospace;word-break:break-all}
.rf-toolbar{display:flex;gap:6px;flex-wrap:wrap;align-items:center}
.rf-btn{border:1px solid rgba(255,255,255,.16);border-radius:6px;background:rgba(255,255,255,.06);
  color:rgba(255,255,255,.85);padding:3px 10px;font-size:11px;cursor:pointer}
.rf-btn:hover{background:rgba(255,255,255,.12)}
.rf-canvas-wrap{height:clamp(240px,55vh,520px);border:1px solid rgba(255,255,255,.1);border-radius:8px;overflow:hidden;position:relative}
.rf-canvas-wrap canvas{position:absolute;inset:0;width:100%;height:100%;display:block;touch-action:none;cursor:grab}
.rf-stat{color:rgba(255,255,255,.4);font-size:11px}
.rf-table{width:100%;border-collapse:collapse;font-family:ui-monospace,Consolas,monospace;font-size:11px}
.rf-table td,.rf-table th{padding:2.5px 8px;border-bottom:1px solid rgba(255,255,255,.07);text-align:left;white-space:nowrap}
.rf-table th{color:rgba(255,255,255,.4);font-weight:600;position:sticky;top:0;background:#14161c}
.rf-table tr.rf-row{cursor:pointer}
.rf-table tr.rf-row:hover{background:rgba(255,255,255,.06)}
.rf-pre{margin:0;padding:8px 10px;background:rgba(0,0,0,.28);border-radius:8px;overflow:auto;
  font:11px/1.45 ui-monospace,Consolas,monospace;white-space:pre;max-height:340px}
.rf-note{color:rgba(255,255,255,.4);font-size:11px}
.rf-chart{border:1px solid rgba(255,255,255,.1);border-radius:8px}
`;
(function injectCss() {
  const style = document.createElement('style');
  style.textContent = CSS;
  document.head.append(style);
})();

/* ---------- 3D 模型 ---------- */
function renderModel(body, st) {
  const model = parseModel(st.title || st.path, st.richBytes);
  const wrap = el('div', { class: 'rf-root' });
  if (model.error) {
    wrap.append(el('div', { class: 'rf-note', text: '解析失败：' + (model.error || '未知错误') }));
    body.append(wrap);
    return () => {};
  }
  const stat = el('span', { class: 'rf-stat', text:
    `顶点 ${model.counts.vertices} · 三角形 ${model.counts.triangles}` + (model.meta && model.meta.units ? ` · 单位 ${model.meta.units}` : '') });
  const canvasWrap = el('div', { class: 'rf-canvas-wrap' });
  const canvas = el('canvas');
  canvasWrap.append(canvas);
  const toolbar = el('div', { class: 'rf-toolbar' });
  wrap.append(toolbar, canvasWrap, el('div', { class: 'rf-box' }, stat, model.warnings && model.warnings.length
    ? el('div', { class: 'rf-note', text: model.warnings.join('；') }) : null));
  body.append(wrap);
  const view = createModelView({ canvas, model });
  let wireOn = false;
  const wireBtn = el('button', { class: 'rf-btn', text: '线框' });
  wireBtn.addEventListener('click', () => {
    wireOn = !wireOn;
    view.setWireframe(wireOn);
    wireBtn.style.background = wireOn ? 'rgba(110,168,255,.28)' : '';
  });
  toolbar.append(wireBtn, el('button', { class: 'rf-btn', text: '重置视角', onclick: () => view.resetView() }));
  return () => view.destroy();
}

/* ---------- 压缩包 ---------- */
async function renderArchive(body, st) {
  const root = el('div', { class: 'rf-root' });
  const scroll = el('div', { class: 'rf-scroll' });
  const detail = el('div');
  root.append(scroll, detail);
  body.append(root);
  let parsed;
  try {
    parsed = await listArchive(st.richBytes, { filename: st.title || st.path });
  } catch (e) {
    root.innerHTML = '';
    root.append(el('div', { class: 'rf-note', text: '解析失败：' + (e && e.message ? e.message : String(e)) }));
    return () => {};
  }
  if (!parsed.supported) {
    root.innerHTML = '';
    root.append(el('div', { class: 'rf-note', text: '不支持的压缩格式：' + (parsed.hint || parsed.format) }));
    return () => {};
  }
  const MAX_ROWS = 500;
  const rows = parsed.entries.slice(0, MAX_ROWS);
  const notes = el('div', { class: 'rf-note', text:
    `${parsed.format} · ${parsed.entries.length} 项` + (parsed.truncated ? `（已截断，显示前 ${MAX_ROWS}）` : '') });
  const table = el('table', { class: 'rf-table' });
  table.append(el('tr', null, el('th', { text: '名称' }), el('th', { text: '大小' }), el('th', { text: '修改时间' })));
  for (const entry of rows) {
    const tr = el('tr', { class: entry.dir ? '' : 'rf-row' },
      el('td', { text: (entry.dir ? '▸ ' : '') + entry.path }),
      el('td', { text: entry.dir ? '—' : formatBytes(entry.size) }),
      el('td', { text: entry.mtime ? new Date(entry.mtime).toLocaleString() : '—' }));
    if (!entry.dir) {
      tr.addEventListener('click', async () => {
        detail.innerHTML = '';
        try {
          const bytes = st.richBytes;
          const read = await readEntryText(bytes, entry, { maxBytes: 512 * 1024 });
          if (read.looksBinary) {
            detail.append(el('div', { class: 'rf-box-t', text: '条目内容：' + entry.path }),
              el('div', { class: 'rf-note', text: `二进制条目（${formatBytes(read.bytes.length)}），不支持文本显示` }));
            return;
          }
          const text = read.text || '';
          const pre = el('pre', { class: 'rf-pre', text: read.truncated ? text + '\n…（已截断）' : text });
          detail.append(el('div', { class: 'rf-box-t', text: '条目内容：' + entry.path }), pre);
        } catch (e) {
          detail.append(el('div', { class: 'rf-note', text: '条目不可读为文本：' + (e && e.message ? e.message : String(e)) }));
        }
      });
    }
    table.append(tr);
  }
  scroll.append(notes, table);
  return () => {};
}

/* ---------- TOML ---------- */
function renderToml(body, st) {
  const text = new TextDecoder().decode(st.richBytes);
  const r = parseToml(text);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  if (!r.ok) { wrap.append(el('div', { class: 'rf-note', text: (r.error || '解析失败') + (r.line ? `（第 ${r.line} 行）` : '') })); body.append(wrap); return () => {}; }
  wrap.append(kvBox('TOML 结构',
    kv('键', r.counts ? r.counts.keys : 0),
    kv('表', r.counts ? r.counts.tables : 0),
    kv('表数组', r.counts ? r.counts.arrays : 0),
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: r.warnings.join('；') }) : null));
  for (const sec of (r.sections || []).slice(0, 60)) {
    const table = el('table', { class: 'rf-table' });
    table.append(el('tr', null, el('th', { text: '键' }), el('th', { text: '类型' }), el('th', { text: '值' })));
    for (const row of sec.rows.slice(0, 200)) {
      table.append(el('tr', null,
        el('td', { text: row.key }),
        el('td', { text: row.type || '' }),
        el('td', { text: row.text })));
    }
    wrap.append(kvBox(sec.header || '（根）', table));
  }
  body.append(wrap);
  return () => {};
}

/* ---------- Intel HEX ---------- */
function renderIhex(body, st) {
  const text = new TextDecoder().decode(st.richBytes);
  const r = parseIntelHex(text);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  if (!r.ok) { wrap.append(el('div', { class: 'rf-note', text: r.error || '解析失败' })); body.append(wrap); return () => {}; }
  wrap.append(kvBox('Intel HEX 概要',
    kv('记录数', r.records),
    kv('数据字节', r.dataBytes),
    kv('地址范围', r.start != null && r.end != null ? `0x${r.start.toString(16).toUpperCase()} – 0x${r.end.toString(16).toUpperCase()}` : null),
    kv('入口地址', r.entry != null ? '0x' + r.entry.toString(16).toUpperCase() : null),
    kv('EOF 记录', r.eof ? '有' : '无'),
    r.errors && r.errors.length ? el('div', { class: 'rf-note', text: '错误：' + r.errors.join('；') }) : null,
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: '警告：' + r.warnings.join('；') }) : null));
  if (r.ranges && r.ranges.length) {
    const table = el('table', { class: 'rf-table' });
    table.append(el('tr', null, el('th', { text: '起始地址' }), el('th', { text: '长度' })));
    for (const rg of r.ranges.slice(0, 100)) {
      table.append(el('tr', null,
        el('td', { text: '0x' + rg.address.toString(16).toUpperCase() }),
        el('td', { text: String(rg.length) + ' B' })));
    }
    wrap.append(kvBox('数据区段', table));
    const first = r.ranges[0];
    if (first.data && first.data.length) {
      wrap.append(kvBox('首段十六进制转储（前 512 B）', el('pre', { class: 'rf-pre', text: formatHexDump(first.data, first.address, 512).join('\n') })));
    }
  }
  body.append(wrap);
  return () => {};
}

/* ---------- JCAMP-DX ---------- */
function renderJcamp(body, st) {
  const text = new TextDecoder().decode(st.richBytes);
  const r = parseJcamp(text);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  if (!r.ok) { wrap.append(el('div', { class: 'rf-note', text: r.error || '解析失败' })); body.append(wrap); return () => {}; }
  wrap.append(kvBox('JCAMP-DX 概要',
    kv('TITLE', r.title), kv('DATA TYPE', r.dataType),
    kv('X 单位', r.xUnits), kv('Y 单位', r.yUnits),
    kv('数据点', r.plotted != null ? r.plotted : r.npoints),
    r.xMin != null ? kv('X 范围', `${r.xMin} – ${r.xMax}`) : null,
    r.yMin != null ? kv('Y 范围', `${r.yMin} – ${r.yMax}`) : null,
    r.origin ? kv('ORIGIN', r.origin) : null,
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: r.warnings.join('；') }) : null));
  if (r.x && r.x.length > 1) {
    const N = Math.min(r.x.length, 4000);
    const W = 560, H = 220;
    const canvas = el('canvas', { class: 'rf-chart', width: W * 2, height: H * 2 });
    canvas.style.width = W + 'px'; canvas.style.height = H + 'px';
    const ctx = canvas.getContext('2d');
    ctx.scale(2, 2);
    ctx.fillStyle = '#14161c'; ctx.fillRect(0, 0, W, H);
    let xMin = Infinity, xMax = -Infinity, yMin = Infinity, yMax = -Infinity;
    for (let i = 0; i < N; i++) {
      if (r.x[i] < xMin) xMin = r.x[i]; if (r.x[i] > xMax) xMax = r.x[i];
      if (r.y[i] < yMin) yMin = r.y[i]; if (r.y[i] > yMax) yMax = r.y[i];
    }
    const sx = (x) => 44 + (W - 54) * ((x - xMin) / Math.max(1e-12, xMax - xMin));
    const sy = (y) => H - 26 - (H - 44) * ((y - yMin) / Math.max(1e-12, yMax - yMin));
    ctx.strokeStyle = 'rgba(255,255,255,.14)';
    ctx.beginPath(); ctx.moveTo(44, 12); ctx.lineTo(44, H - 26); ctx.lineTo(W - 10, H - 26); ctx.stroke();
    ctx.strokeStyle = '#6ea8ff'; ctx.lineWidth = 1.2;
    ctx.beginPath();
    for (let i = 0; i < N; i++) {
      const px = sx(r.x[i]), py = sy(r.y[i]);
      if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py);
    }
    ctx.stroke();
    ctx.fillStyle = 'rgba(255,255,255,.45)'; ctx.font = '10px ui-monospace,Consolas,monospace';
    ctx.fillText(String(xMin), 44, H - 12); ctx.fillText(String(xMax), W - 46, H - 12);
    ctx.fillText(String(yMin), 4, H - 28); ctx.fillText(String(yMax), 4, 20);
    wrap.append(kvBox('谱图（降采样 ≤4000 点）', canvas));
    if (r.downsampled) wrap.append(el('div', { class: 'rf-note', text: '数据段已降采样' }));
  }
  body.append(wrap);
  return () => {};
}

/* ---------- VCD ---------- */
function renderVcd(body, st) {
  const text = new TextDecoder().decode(st.richBytes);
  const r = parseVcd(text);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  wrap.append(kvBox('VCD 概要',
    kv('时间刻度', r.timescale ? r.timescale.text : null),
    kv('日期', r.date), kv('版本', r.version),
    kv('信号数', r.signals ? r.signals.length : 0),
    kv('结束时间', r.endTime),
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: r.warnings.join('；') }) : null,
    r.truncated ? el('div', { class: 'rf-note', text: '解析截断' }) : null));
  if (r.signals && r.signals.length) {
    const table = el('table', { class: 'rf-table' });
    table.append(el('tr', null, el('th', { text: 'ID' }), el('th', { text: '名称' }), el('th', { text: '位宽' })));
    for (const s of r.signals.slice(0, 300)) {
      table.append(el('tr', null,
        el('td', { text: s.id || '—' }),
        el('td', { text: s.name || s.reference || '—' }),
        el('td', { text: s.width != null ? String(s.width) : '—' })));
    }
    wrap.append(kvBox('信号表（前 300 个）', table));
  }
  body.append(wrap);
  return () => {};
}

/* ---------- KiCad ---------- */
function renderKicad(body, st) {
  const text = new TextDecoder().decode(st.richBytes);
  const r = parseKicad(text);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  if (!r.ok) { wrap.append(el('div', { class: 'rf-note', text: r.error || '解析失败' })); body.append(wrap); return () => {}; }
  const stats = r.stats && typeof r.stats === 'object' ? r.stats : {};
  const statPairs = Object.entries(stats).slice(0, 12).map(([k, v]) => kv(k, typeof v === 'object' ? JSON.stringify(v) : v));
  wrap.append(kvBox(`KiCad ${r.kind || '文档'}`,
    kv('版本', r.version), kv('标题', r.title), kv('图幅', r.paper),
    ...statPairs,
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: r.warnings.join('；') }) : null));
  body.append(wrap);
  return () => {};
}

/* ---------- DFU ---------- */
function renderDfu(body, st) {
  const r = parseDfu(st.richBytes);
  const wrap = el('div', { class: 'rf-root rf-scroll' });
  if (!r.ok) { wrap.append(el('div', { class: 'rf-note', text: r.error || '解析失败' })); body.append(wrap); return () => {}; }
  const hex = (n) => '0x' + n.toString(16).toUpperCase().padStart(4, '0');
  wrap.append(kvBox('DFU 固件概要',
    kv('格式', r.dfuse ? `DfuSe v${r.dfuseVersion}` : '标准 DFU'),
    kv('VID / PID', `${hex(r.vendorId)} / ${hex(r.productId)}` + (r.vendor || r.product ? `（${r.vendor || ''} ${r.product || ''}）` : '')),
    kv('设备版本', r.bcdDevice), kv('DFU 版本', r.bcdDfu),
    kv('目标数', r.targets ? r.targets.length : 0),
    kv('负载字节', r.payloadBytes != null ? formatBytes(r.payloadBytes) : null),
    kv('CRC 校验', r.crcOk ? '通过' : '失败'),
    r.warnings && r.warnings.length ? el('div', { class: 'rf-note', text: r.warnings.join('；') }) : null));
  if (r.targets && r.targets.length) {
    const table = el('table', { class: 'rf-table' });
    table.append(el('tr', null, el('th', { text: 'Alt' }), el('th', { text: '名称' }), el('th', { text: '大小' }), el('th', { text: '元素' })));
    for (const t of r.targets.slice(0, 50)) {
      let size = 0;
      for (const e of t.elements || []) size += e.size || 0;
      table.append(el('tr', null,
        el('td', { text: String(t.alt ?? '—') }),
        el('td', { text: t.name || '—' }),
        el('td', { text: size > 0 ? formatBytes(size) : '—' }),
        el('td', { text: String(t.elements ? t.elements.length : 0) })));
    }
    wrap.append(kvBox('目标（前 50 个）', table));
    const firstEl = r.targets[0] && r.targets[0].elements && r.targets[0].elements[0];
    if (firstEl && firstEl.preview && firstEl.preview.length) {
      wrap.append(kvBox('首元素头 48 字节', el('pre', { class: 'rf-pre', text: formatHexDump(firstEl.preview, 0, 48).join('\n') })));
    }
  }
  body.append(wrap);
  return () => {};
}

/* ---------- 挂载入口 ---------- */
const RENDERERS = {
  model: renderModel,
  archive: renderArchive,
  toml: renderToml,
  ihex: renderIhex,
  jcamp: renderJcamp,
  vcd: renderVcd,
  kicad: renderKicad,
  dfu: renderDfu,
};

/** 挂载富预览正文：返回 dispose()。正文渲染失败显示可读错误，不白屏。 */
async function mount(body, kind, st) {
  const render = RENDERERS[kind];
  if (!render) {
    body.append(el('div', { class: 'rf-note', text: `未知富预览类型：${kind}` }));
    return () => {};
  }
  try {
    return await render(body, st) || (() => {});
  } catch (e) {
    body.innerHTML = '';
    body.append(el('div', { class: 'rf-note', text: '预览渲染失败：' + (e && e.message ? e.message : String(e)) }));
    return () => {};
  }
}

window.RichPreview = { extKind, extOf, mount };
