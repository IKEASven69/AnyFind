window.__ModuleLoader__.load({
	id: "dsh-recall",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		"use strict";
		var __defProp = Object.defineProperty;
		var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
		var __getOwnPropNames = Object.getOwnPropertyNames;
		var __hasOwnProp = Object.prototype.hasOwnProperty;
		var __export = (target, all) => {
		  for (var name in all)
		    __defProp(target, name, { get: all[name], enumerable: true });
		};
		var __copyProps = (to, from, except, desc) => {
		  if (from && typeof from === "object" || typeof from === "function") {
		    for (let key of __getOwnPropNames(from))
		      if (!__hasOwnProp.call(to, key) && key !== except)
		        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
		  }
		  return to;
		};
		var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);
		
		// src/client.ts
		var client_exports = {};
		__export(client_exports, {
		  apply: () => apply,
		  inject: () => inject
		});
		module.exports = __toCommonJS(client_exports);
		var import_react = require("react");
		
		// src/core.ts
		var KINDS = ["file", "content", "session", "skill", "memory"];
		var KIND_LABEL = {
		  file: "\u6587\u4EF6",
		  content: "\u5185\u5BB9",
		  session: "\u4F1A\u8BDD",
		  skill: "\u6280\u80FD",
		  memory: "\u8BB0\u5FC6"
		};
		function escapeWithMark(text) {
		  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/&lt;mark&gt;/g, "<mark>").replace(/&lt;\/mark&gt;/g, "</mark>");
		}
		function fmtMtime(mtime) {
		  if (mtime === null || mtime === "") return "";
		  const d = new Date(mtime);
		  if (Number.isNaN(d.getTime()) || d.getFullYear() < 1990) return "";
		  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
		}
		
		// src/client.ts
		var inject = ["slots"];
		function countRemainingTabs(node) {
		  if (node === null || typeof node !== "object") return 1;
		  const n = node;
		  if (Array.isArray(n.tabs)) return n.tabs.length;
		  const kids = [n.children, n.left, n.right].filter(Array.isArray).flat();
		  if (kids.length === 0) return 1;
		  return kids.reduce((sum, k) => sum + countRemainingTabs(k), 0);
		}
		var betterSidebarSvc = null;
		var disposeDirectTab = null;
		var overlayStore = {
		  open: false,
		  listeners: /* @__PURE__ */ new Set(),
		  toggle() {
		    overlayStore.open = !overlayStore.open;
		    for (const l of overlayStore.listeners) l();
		  },
		  show() {
		    if (overlayStore.open) return;
		    overlayStore.open = true;
		    for (const l of overlayStore.listeners) l();
		  },
		  close() {
		    if (!overlayStore.open) return;
		    overlayStore.open = false;
		    for (const l of overlayStore.listeners) l();
		  },
		  subscribe(fn) {
		    overlayStore.listeners.add(fn);
		    return () => {
		      overlayStore.listeners.delete(fn);
		    };
		  }
		};
		var CSS = `
		.rc-panel { display: flex; flex-direction: column; width: 100%; height: 100%; min-height: 0;
		  background: var(--dsw-alias-bg-base, #fff); color: var(--dsw-alias-label-primary, inherit);
		  --rc-a: var(--dsw-alias-brand-primary, var(--accent, #2563eb));
		  --rc-line: var(--dsw-alias-border-l2, var(--border, rgba(127,127,127,.28)));
		  --rc-mut: var(--dsw-alias-label-secondary, var(--muted, rgba(127,127,127,.92)));
		  --rc-card: var(--dsw-alias-bg-layer-1, var(--bg, rgba(127,127,127,.05)));
		  --rc-err: var(--dsw-alias-state-error-primary, #d93025);
		  --rc-ok: var(--dsw-alias-state-success-primary, #15803d);
		  --rc-f-name: #6ea8ff; --rc-f-val: #2dd4bf; --rc-f-phrase: #fbbf24;
		  --rc-f-excl: #f87171; --rc-f-op: #a78bfa; }
		.rc-head { display: flex; gap: 8px; padding: 14px 14px 10px; flex: none; position: relative; z-index: 20; }
		
		/* \u641C\u7D22\u6846\uFF1A\u900F\u660E input \u53E0 token \u67D3\u8272\u80CC\u677F\uFF08\u684C\u9762\u7248 DSL \u67D3\u8272\u601D\u8DEF\u7684 React \u79FB\u690D\uFF09 */
		.rc-input-wrap { position: relative; flex: 1; min-width: 0; background: var(--rc-card); border-radius: 10px; }
		.rc-input { width: 100%; box-sizing: border-box; font-size: 14px; line-height: 22px; padding: 9px 36px 9px 14px;
		  border-radius: 10px; border: 1px solid var(--rc-line); background: transparent; outline: none;
		  position: relative; z-index: 1; color: transparent; caret-color: var(--dsw-alias-label-primary, #888);
		  font-family: inherit; letter-spacing: normal; }
		.rc-input::placeholder { color: var(--rc-mut); }
		.rc-input:focus { border-color: var(--rc-a); }
		.rc-input-hl { position: absolute; inset: 1px; padding: 9px 36px 9px 14px; font-size: 14px; line-height: 22px;
		  font-family: inherit; letter-spacing: normal; white-space: pre; overflow: hidden; pointer-events: none;
		  color: var(--dsw-alias-label-primary, inherit); z-index: 0; }
		.rc-tk-name { color: var(--rc-f-name); font-weight: 600; }
		.rc-tk-val { color: var(--rc-f-val); }
		.rc-tk-phrase { color: var(--rc-f-phrase); }
		.rc-tk-excl { color: var(--rc-f-excl); text-decoration: line-through; text-decoration-thickness: 1px; }
		.rc-tk-op { color: var(--rc-f-op); font-weight: 700; }
		.rc-syn-btn { position: absolute; right: 8px; top: 50%; transform: translateY(-50%); z-index: 2;
		  width: 24px; height: 24px; border-radius: 7px; border: none; cursor: pointer;
		  background: transparent; color: var(--rc-mut); font-size: 13px; font-weight: 600; }
		.rc-syn-btn:hover { background: rgba(127,127,127,.15); color: var(--dsw-alias-label-primary, inherit); }
		
		.rc-btn { cursor: pointer; border-radius: 9px; font-size: 12.5px; padding: 7px 14px; white-space: nowrap;
		  border: 1px solid var(--rc-line); background: transparent; color: inherit; }
		.rc-btn:hover { border-color: var(--rc-a); color: var(--rc-a); }
		.rc-btn-pri { background: var(--rc-a); border-color: transparent; color: #fff; }
		.rc-btn-pri:hover { color: #fff; opacity: .92; }
		.rc-btn:disabled { opacity: .5; cursor: default; }
		
		/* kind \u5206\u6BB5\u63A7\u4EF6\uFF08desktop.html \u79FB\u690D\uFF1A\u73BB\u7483\u5BB9\u5668 + \u6ED1\u52A8\u6307\u793A\u5668 + \u56FE\u6807 + \u8BA1\u6570\u89D2\u6807\uFF1B\u8D85\u5BBD\u6A2A\u5411\u6EDA\u52A8\u515C\u5E95\uFF09 */
		.rc-seg-scroll { overflow-x: auto; scrollbar-width: none; flex: none; padding: 0 14px 10px; }
		.rc-seg-scroll::-webkit-scrollbar { display: none; }
		.rc-seg { --rc-accent: #9db4e8; position: relative; display: flex; width: max-content; min-width: 100%;
		  box-sizing: border-box; padding: 3px; border-radius: 12px;
		  background: var(--dsw-alias-bg-layer-1, rgba(127,127,127,.06)); border: 1px solid var(--rc-line);
		  box-shadow: inset 0 1px 0 rgba(255,255,255,.05), inset 0 -2px 4px rgba(0,0,0,.06); }
		.rc-seg-ind { position: absolute; top: 3px; bottom: 3px; left: 3px; width: 0; border-radius: 9px;
		  background: color-mix(in srgb, var(--rc-accent) 24%, transparent);
		  box-shadow: 0 0 12px color-mix(in srgb, var(--rc-accent) 28%, transparent),
		    inset 0 0 0 1px color-mix(in srgb, var(--rc-accent) 45%, transparent);
		  transition: left .28s cubic-bezier(.32,.72,.28,1), width .28s cubic-bezier(.32,.72,.28,1), background .25s, box-shadow .25s;
		  pointer-events: none; }
		.rc-seg-btn { --sa: #9db4e8; position: relative; z-index: 1; flex: 1 0 auto; display: flex; align-items: center;
		  justify-content: center; gap: 5px; padding: 5px 10px; font-size: 12px; font-weight: 500; font-family: inherit;
		  letter-spacing: .2px; background: transparent; border: none; border-radius: 9px; cursor: pointer;
		  color: var(--rc-mut); transition: color .18s; white-space: nowrap; }
		.rc-seg-btn:hover { color: var(--dsw-alias-label-primary, inherit); }
		.rc-seg-btn.active { color: var(--dsw-alias-label-primary, inherit); font-weight: 600; }
		.rc-seg-btn.active .rc-seg-ico { color: var(--sa); }
		.rc-seg-ico { width: 13px; height: 13px; flex-shrink: 0; }
		.rc-seg-count { font-size: 9px; font-variant-numeric: tabular-nums; letter-spacing: .5px; line-height: 1;
		  color: var(--rc-mut); background: rgba(127,127,127,.12); padding: 2.5px 5px; border-radius: 7px; }
		.rc-seg-btn.active .rc-seg-count { color: var(--sa); background: rgba(0,0,0,.18); }
		.rc-seg-count:empty { display: none; }
		
		.rc-body { flex: 1; overflow-y: auto; padding: 4px 14px 14px; display: flex; flex-direction: column; gap: 10px;
		  position: relative; }
		
		/* \u52A0\u8F7D\u6001\uFF1A\u7ED3\u679C\u533A\u5C45\u4E2D + skeleton \u5360\u4F4D\u884C */
		.rc-loading { display: flex; flex-direction: column; align-items: center; justify-content: center;
		  gap: 14px; flex: 1; min-height: 160px; width: 100%; }
		.rc-loading-t { font-size: 12.5px; color: var(--rc-mut); display: flex; align-items: center; gap: 8px; }
		.rc-spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid rgba(127,127,127,.3);
		  border-top-color: var(--rc-a); animation: rcSpin .7s linear infinite; }
		@keyframes rcSpin { to { transform: rotate(360deg); } }
		.rc-skel { width: 100%; max-width: 520px; display: flex; flex-direction: column; gap: 10px; }
		.rc-skel-row { height: 34px; border-radius: 9px; background: var(--rc-card); border: 1px solid var(--rc-line);
		  position: relative; overflow: hidden; }
		.rc-skel-row::after { content: ''; position: absolute; inset: 0;
		  background: linear-gradient(90deg, transparent, rgba(127,127,127,.16), transparent);
		  animation: rcShimmer 1.3s ease infinite; }
		.rc-skel-row:nth-child(2) { width: 88%; } .rc-skel-row:nth-child(3) { width: 72%; }
		@keyframes rcShimmer { from { transform: translateX(-100%); } to { transform: translateX(100%); } }
		
		/* \u8BED\u6CD5\u5361\u5F39\u5C42 */
		.rc-syn { position: absolute; top: calc(100% + 2px); right: 14px; z-index: 40; width: min(380px, calc(100% - 28px));
		  border-radius: 12px; padding: 12px 14px; background: var(--dsw-alias-bg-layer-2, var(--dsw-alias-bg-base, #fff));
		  border: 1px solid var(--rc-line); box-shadow: 0 12px 40px rgba(0,0,0,.28);
		  display: flex; flex-direction: column; gap: 2px; animation: rcIn .18s ease both; }
		.rc-syn-t { font-size: 12px; font-weight: 700; margin-bottom: 6px; }
		.rc-syn-row { display: flex; gap: 10px; align-items: baseline; padding: 3px 4px; border-radius: 7px; cursor: pointer; }
		.rc-syn-row:hover { background: rgba(127,127,127,.1); }
		.rc-syn-row code { font-size: 11.5px; font-weight: 700; color: var(--rc-f-name); flex: none; min-width: 74px; }
		.rc-syn-row span { font-size: 11px; color: var(--rc-mut); }
		.rc-syn-ex { font-size: 11px; color: var(--rc-mut); margin-top: 8px; border-top: 1px solid var(--rc-line); padding-top: 8px; }
		.rc-syn-ex code { color: var(--rc-f-val); user-select: all; }
		
		.rc-group { display: flex; flex-direction: column; gap: 6px; }
		.rc-group-t { font-size: 12px; font-weight: 700; color: var(--rc-mut); display: flex; gap: 6px; align-items: center; }
		.rc-hit { border: 1px solid var(--rc-line); border-radius: 10px; padding: 9px 12px; background: var(--rc-card);
		  display: flex; flex-direction: column; gap: 4px; animation: rcIn .25s ease both; }
		.rc-hit.focused { border-color: var(--rc-a); box-shadow: 0 0 0 1px color-mix(in srgb, var(--rc-a) 55%, transparent); }
		@keyframes rcIn { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: none; } }
		.rc-hit-t { font-size: 12.5px; font-weight: 600; word-break: break-all; }
		.rc-hit-meta { font-size: 10.5px; color: var(--rc-mut); display: flex; gap: 8px; flex-wrap: wrap; }
		.rc-hit-s { font-size: 12px; line-height: 1.55; color: var(--rc-mut); word-break: break-word; }
		.rc-hit-s mark { background: rgba(250, 204, 21, .35); color: inherit; border-radius: 3px; padding: 0 1px; }
		.rc-hit-path { font-size: 10.5px; color: var(--rc-mut); word-break: break-all; font-family: monospace; }
		.rc-resume { align-self: flex-start; font-size: 11px; padding: 3px 10px; }
		.rc-banner { font-size: 12px; line-height: 1.65; border-radius: 9px; padding: 9px 12px; }
		.rc-banner-err { color: var(--rc-err); background: rgba(211,47,47,.08); }
		.rc-banner-info { color: var(--rc-mut); background: rgba(127,127,127,.08); }
		.rc-guide { display: flex; flex-direction: column; gap: 12px; padding: 4px 2px; }
		.rc-guide-t { font-size: 14px; font-weight: 700; }
		.rc-cmd { font-family: monospace; font-size: 11.5px; background: rgba(0,0,0,.08); border-radius: 8px;
		  padding: 8px 10px; word-break: break-all; user-select: all; white-space: pre-wrap; }
		.rc-foot { flex: none; border-top: 1px solid var(--rc-line); padding: 8px 12px; display: flex; gap: 8px; align-items: center; }
		.rc-foot input { flex: 1; min-width: 0; font-size: 11px; padding: 4px 8px; border-radius: 7px;
		  border: 1px solid var(--rc-line); background: transparent; color: inherit; outline: none; font-family: monospace; }
		.rc-foot .rc-btn { font-size: 11px; padding: 4px 10px; }
		.rc-meta-pill { font-size: 10.5px; color: var(--rc-ok); font-weight: 600; }
		.rc-kbd { font-size: 10px; color: var(--rc-mut); border: 1px solid var(--rc-line); border-radius: 5px;
		  padding: 1px 6px; font-family: inherit; }
		.rc-toast { position: absolute; left: 50%; bottom: 52px; transform: translateX(-50%); z-index: 50;
		  font-size: 11.5px; padding: 5px 14px; border-radius: 999px; color: #fff; background: rgba(30,32,40,.92);
		  box-shadow: 0 6px 20px rgba(0,0,0,.3); animation: rcIn .18s ease both; pointer-events: none; }
		
		/* Spotlight \u6D6E\u5C42\uFF1A\u906E\u7F69 blur + \u5C45\u4E2D\u73BB\u7483\u6846 */
		.rc-mask { position: fixed; inset: 0; z-index: 90; background: rgba(8,10,16,.46);
		  backdrop-filter: blur(10px) saturate(140%); -webkit-backdrop-filter: blur(10px) saturate(140%);
		  display: flex; align-items: flex-start; justify-content: center; padding-top: 11vh;
		  animation: rcFade .16s ease both; }
		.rc-modal { width: 680px; max-width: 90vw; height: min(72vh, 660px); display: flex; flex-direction: column;
		  border-radius: 16px; overflow: hidden; background: var(--dsw-alias-bg-base, #fff);
		  border: 1px solid var(--rc-line); box-shadow: 0 24px 80px rgba(0,0,0,.42), inset 0 1px 0 rgba(255,255,255,.06);
		  animation: rcPop .18s cubic-bezier(.3,1.15,.4,1) both; }
		@keyframes rcFade { from { opacity: 0; } to { opacity: 1; } }
		@keyframes rcPop { from { opacity: 0; transform: translateY(-10px) scale(.98); } to { opacity: 1; transform: none; } }
		
		/* \u5360\u4F4D tab\uFF08\u4FA7\u8FB9\u5361\u7247\u5165\u53E3\u70B9\u51FB \u2192 \u547C\u51FA\u6D6E\u5C42 + \u63D0\u793A\uFF09 */
		.rc-ph { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 14px;
		  padding: 40px 24px; text-align: center; color: var(--rc-mut); }
		.rc-ph-t { font-size: 14px; font-weight: 700; color: var(--dsw-alias-label-primary, inherit); }
		.rc-ph kbd { font-size: 11px; border: 1px solid var(--rc-line); border-bottom-width: 2px; border-radius: 6px;
		  padding: 2px 8px; background: var(--rc-card); color: var(--dsw-alias-label-primary, inherit); font-family: inherit; }
		
		@media (prefers-reduced-motion: reduce) { .rc-hit, .rc-syn, .rc-mask, .rc-modal, .rc-toast { animation: none !important; }
		  .rc-seg-ind { transition: none !important; } .rc-skel-row::after, .rc-spin { animation: none !important; } }
		`;
		function StyleTag() {
		  return (0, import_react.createElement)("style", null, CSS);
		}
		var CoreDown = class extends Error {
		  /** down 响应里宿主带回的当前配置地址（引导页/地址栏展示用）。 */
		  endpoint;
		};
		function coreDown(body, fallback) {
		  const err = new CoreDown(body.error ?? fallback);
		  if (typeof body.endpoint === "string") err.endpoint = body.endpoint;
		  return err;
		}
		async function getHealth() {
		  const res = await fetch("/dsh-anyfind/api/health", { cache: "no-store" });
		  const body = await res.json();
		  if (!res.ok || body.down === true) throw coreDown(body, `HTTP ${res.status}`);
		  return body;
		}
		async function getSearch(q) {
		  const url = new URLSearchParams({ q, limit: "10" });
		  const res = await fetch(`/dsh-anyfind/api/search?${url.toString()}`, { cache: "no-store" });
		  const body = await res.json();
		  if (body.down === true) throw coreDown(body, "\u5185\u6838\u672A\u542F\u52A8");
		  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`);
		  return body;
		}
		async function postResume(hit) {
		  const res = await fetch("/dsh-anyfind/api/resume", {
		    method: "POST",
		    headers: { "content-type": "application/json" },
		    body: JSON.stringify({ agent: hit.agent, session_id: hit.session_id, cwd: hit.cwd })
		  });
		  const body = await res.json();
		  if (body.down === true) throw coreDown(body, "\u5185\u6838\u672A\u542F\u52A8");
		  if (!res.ok || !body.ok || typeof body.command !== "string") throw new Error(body.error ?? `HTTP ${res.status}`);
		  return body.command;
		}
		function tintQuery(q) {
		  const out = [];
		  const re = /(\s+)|("[^"]*"?)|(-(?:"[^"]*"|\S+))|\b(OR)\b|((?:kind|agent|project|date|ext|in|role|size):(?:"[^"]*"|\S*))|(\S+)/g;
		  let m;
		  let last = 0;
		  while ((m = re.exec(q)) !== null) {
		    if (m.index > last) out.push({ text: q.slice(last, m.index), cls: null });
		    last = re.lastIndex;
		    const [tok, ws, phrase, excl, op, field] = m;
		    if (ws !== void 0) {
		      out.push({ text: ws, cls: null });
		      continue;
		    }
		    if (phrase !== void 0) {
		      out.push({ text: phrase, cls: "rc-tk-phrase" });
		      continue;
		    }
		    if (excl !== void 0) {
		      out.push({ text: excl, cls: "rc-tk-excl" });
		      continue;
		    }
		    if (op !== void 0) {
		      out.push({ text: op, cls: "rc-tk-op" });
		      continue;
		    }
		    if (field !== void 0) {
		      const ci = tok.indexOf(":");
		      out.push({ text: tok.slice(0, ci + 1), cls: "rc-tk-name" });
		      if (ci + 1 < tok.length) out.push({ text: tok.slice(ci + 1), cls: "rc-tk-val" });
		      continue;
		    }
		    out.push({ text: tok, cls: null });
		  }
		  if (last < q.length) out.push({ text: q.slice(last), cls: null });
		  return out;
		}
		var SYNTAX_ROWS = [
		  { ins: "kind:", desc: "file / content / session / skill / memory" },
		  { ins: "agent:", desc: "claude-code / codex / dsh / opencode / pi / cursor" },
		  { ins: "project:", desc: "\u6309\u9879\u76EE\u540D\uFF0C\u5982 project:filefind" },
		  { ins: "date:", desc: "date:2026-10-01..2026-10-20 \u6216 date:7d" },
		  { ins: "ext:", desc: "\u6309\u6269\u5C55\u540D\uFF0C\u5982 ext:.py\u3001ext:md" },
		  { ins: "in:", desc: "\u9650\u5B9A\u76EE\u5F55\uFF0C\u5982 in:D:/coding" },
		  { ins: "role:", desc: "\u4F1A\u8BDD\u5185\u89D2\u8272\uFF0C\u5982 role:user\uFF08\u4EC5\u4F1A\u8BDD\u7EC4\uFF09" },
		  { ins: "size:", desc: "\u6309\u6587\u4EF6\u5927\u5C0F\uFF0C\u5982 size:>1MB\uFF08\u4EC5\u6587\u4EF6\u7EC4\uFF09" },
		  { ins: '"', desc: '\u77ED\u8BED\u7CBE\u786E\u5339\u914D\uFF0C\u5982 "hello world"' },
		  { ins: "OR", desc: "\u6216\u6761\u4EF6\uFF0C\u5982 nginx OR caddy" },
		  { ins: "-", desc: "\u6392\u9664\u8BCD\uFF0C\u5982 -test -kind:file" }
		];
		var SEG_DEFS = [
		  { k: "", label: "\u5168\u90E8", accent: "#9db4e8", icon: '<rect x="2" y="2" width="4.2" height="4.2" rx="1.2"/><rect x="7.8" y="2" width="4.2" height="4.2" rx="1.2"/><rect x="2" y="7.8" width="4.2" height="4.2" rx="1.2"/><rect x="7.8" y="7.8" width="4.2" height="4.2" rx="1.2"/>' },
		  { k: "file", label: "\u6587\u4EF6", accent: "#6ea8ff", icon: '<path d="M3.8 1.8h4l3.4 3.4v6.2a1.4 1.4 0 0 1-1.4 1.4H3.8a1.4 1.4 0 0 1-1.4-1.4V3.2a1.4 1.4 0 0 1 1.4-1.4z"/><path d="M7.8 1.8v3.4h3.4"/>' },
		  { k: "content", label: "\u5185\u5BB9", accent: "#2dd4bf", icon: '<path d="M2.6 3.6h8.8"/><path d="M2.6 6.6h8.8"/><path d="M2.6 9.6h5.4"/>' },
		  { k: "session", label: "AI \u4F1A\u8BDD", accent: "#f472b6", icon: '<rect x="1.9" y="2.4" width="10.2" height="6.8" rx="2.2"/><path d="M5 9.2l-1.1 2.6 3.4-2.1"/>' },
		  { k: "skill", label: "\u6280\u80FD", accent: "#fbbf24", icon: '<path d="M7.6 1.4L3.2 7.9h3.3l-.9 4.7 5.2-6.8H7.4z"/>' },
		  { k: "memory", label: "\u8BB0\u5FC6", accent: "#a78bfa", icon: '<path d="M7 2.6v8.8"/><path d="M7 3.6C6.5 2.7 5.6 2.1 4.6 2.1 3.2 2.1 2.1 4.6c0 .5.13.9.35 1.3A2.4 2.4 0 0 0 2 8.1c0 1.3 1.1 2.4 2.4 2.4.9 0 1.7-.4 2.1-1.1"/><path d="M7 3.6c.5-.9 1.4-1.5 2.4-1.5 1.4 0 2.5 1.1 2.5 2.5 0 .5-.13.9-.35 1.3.4.6.46 1.3.46 2.2 0 1.3-1.1 2.4-2.4 2.4-.9 0-1.7-.4-2.1-1.1"/>' }
		];
		function fmtCount(n) {
		  if (n === void 0 || n === null || n <= 0) return "";
		  if (n >= 1e8) return `${(n / 1e8).toFixed(1).replace(/\.0$/, "")} \u4EBF`;
		  if (n >= 1e4) return `${(n / 1e4).toFixed(1).replace(/\.0$/, "")} \u4E07`;
		  return String(n);
		}
		function SegControl({ active, counts, onChange }) {
		  const segRef = (0, import_react.useRef)(null);
		  const indRef = (0, import_react.useRef)(null);
		  const btnRefs = (0, import_react.useRef)({});
		  (0, import_react.useLayoutEffect)(() => {
		    const seg = segRef.current;
		    const ind = indRef.current;
		    if (seg === null || ind === null) return;
		    const measure = () => {
		      const btn = btnRefs.current[active];
		      if (btn === null || btn === void 0) return;
		      ind.style.left = `${btn.offsetLeft}px`;
		      ind.style.width = `${btn.offsetWidth}px`;
		      const def = SEG_DEFS.find((d) => d.k === active);
		      if (def !== void 0) seg.style.setProperty("--rc-accent", def.accent);
		    };
		    measure();
		    const ro = new ResizeObserver(measure);
		    ro.observe(seg);
		    return () => {
		      ro.disconnect();
		    };
		  }, [active, counts]);
		  return (0, import_react.createElement)(
		    "div",
		    { className: "rc-seg-scroll" },
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-seg", ref: segRef, role: "tablist" },
		      (0, import_react.createElement)("div", { className: "rc-seg-ind", ref: indRef }),
		      ...SEG_DEFS.map((d) => (0, import_react.createElement)(
		        "button",
		        {
		          key: d.k || "all",
		          className: `rc-seg-btn${active === d.k ? " active" : ""}`,
		          role: "tab",
		          "aria-selected": active === d.k,
		          style: { "--sa": d.accent },
		          ref: (el) => {
		            btnRefs.current[d.k] = el;
		          },
		          onClick: () => {
		            onChange(d.k);
		          },
		          onKeyDown: (e) => {
		            if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
		            e.preventDefault();
		            const i = SEG_DEFS.findIndex((x) => x.k === active);
		            const j = (i + (e.key === "ArrowRight" ? 1 : -1) + SEG_DEFS.length) % SEG_DEFS.length;
		            onChange(SEG_DEFS[j].k);
		            btnRefs.current[SEG_DEFS[j].k]?.focus();
		          }
		        },
		        (0, import_react.createElement)("svg", {
		          className: "rc-seg-ico",
		          viewBox: "0 0 14 14",
		          fill: "none",
		          stroke: "currentColor",
		          strokeWidth: 1.3,
		          strokeLinecap: "round",
		          strokeLinejoin: "round",
		          dangerouslySetInnerHTML: { __html: d.icon }
		        }),
		        (0, import_react.createElement)("span", null, d.label),
		        (0, import_react.createElement)("span", { className: "rc-seg-count" }, counts[d.k] ?? "")
		      ))
		    )
		  );
		}
		function AnyfindMark({ size = 16 }) {
		  return (0, import_react.createElement)(
		    "svg",
		    { width: size, height: size, viewBox: "0 0 24 24", "aria-hidden": true, fill: "none", stroke: "currentColor", strokeWidth: 2.4, strokeLinecap: "round" },
		    (0, import_react.createElement)("circle", { cx: 10.5, cy: 10.5, r: 6.5 }),
		    (0, import_react.createElement)("line", { x1: 15.5, y1: 15.5, x2: 21, y2: 21 })
		  );
		}
		function Snippet({ text }) {
		  return (0, import_react.createElement)("div", { className: "rc-hit-s", dangerouslySetInnerHTML: { __html: escapeWithMark(text) } });
		}
		function HitRow({ hit, focused, cmd, busyKey, onResume, onCopy, onDown }) {
		  const [err, setErr] = (0, import_react.useState)(null);
		  const rowRef = (0, import_react.useRef)(null);
		  (0, import_react.useEffect)(() => {
		    if (focused) rowRef.current?.scrollIntoView({ block: "nearest" });
		  }, [focused]);
		  const meta = [hit.agent, hit.project, fmtMtime(hit.mtime)].filter((x) => x !== null && x !== "");
		  return (0, import_react.createElement)(
		    "div",
		    { className: `rc-hit${focused ? " focused" : ""}`, ref: rowRef },
		    (0, import_react.createElement)("div", { className: "rc-hit-t" }, hit.title ?? hit.path ?? "(\u65E0\u6807\u9898)"),
		    meta.length > 0 ? (0, import_react.createElement)("div", { className: "rc-hit-meta" }, ...meta.map((m, i) => (0, import_react.createElement)("span", { key: i }, m))) : null,
		    hit.snippet !== null && hit.snippet !== "" ? (0, import_react.createElement)(Snippet, { text: hit.snippet }) : null,
		    hit.path !== null && hit.path !== hit.title ? (0, import_react.createElement)("div", { className: "rc-hit-path" }, hit.path) : null,
		    hit.session_id !== null && hit.agent !== null ? (0, import_react.createElement)("button", {
		      className: "rc-btn rc-resume",
		      disabled: busyKey,
		      onClick: () => {
		        void postResume(hit).then(
		          (command) => {
		            onResume({ ...hit });
		            onCopy(command, "\u5DF2\u590D\u5236 resume \u547D\u4EE4");
		          },
		          (e) => {
		            if (e instanceof CoreDown) {
		              onDown(e.endpoint);
		              return;
		            }
		            setErr(e instanceof Error ? e.message : String(e));
		          }
		        );
		      }
		    }, busyKey ? "\u751F\u6210\u4E2D\u2026" : "\u21A9 \u8DF3\u56DE\u4F1A\u8BDD") : null,
		    err !== null ? (0, import_react.createElement)("div", { className: "rc-banner rc-banner-err" }, err) : null,
		    cmd !== null ? (0, import_react.createElement)(
		      "div",
		      { style: { display: "flex", gap: 6, alignItems: "center" } },
		      (0, import_react.createElement)("span", { className: "rc-cmd", style: { flex: 1 } }, cmd),
		      (0, import_react.createElement)("button", {
		        className: "rc-btn",
		        style: { fontSize: 11, padding: "3px 10px" },
		        onClick: () => {
		          onCopy(cmd, "\u5DF2\u590D\u5236");
		        }
		      }, "\u590D\u5236")
		    ) : null
		  );
		}
		function DownGuide({ detail, onRetry }) {
		  return (0, import_react.createElement)(
		    "div",
		    { className: "rc-guide" },
		    (0, import_react.createElement)("div", { className: "rc-guide-t" }, "\u641C\u7D22\u670D\u52A1\u672A\u8FDE\u63A5"),
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-banner rc-banner-info" },
		      "\u8BF7\u5148\u542F\u52A8 AnyFind \u684C\u9762\u7248\u3002\u672C\u5730\u641C\u7D22\u670D\u52A1\u4F1A\u81EA\u52A8\u542F\u52A8\uFF0C\u65E0\u9700\u914D\u7F6E\u5730\u5740\u6216\u7AEF\u53E3\u3002"
		    ),
		    detail !== "" ? (0, import_react.createElement)("div", { className: "rc-banner rc-banner-err" }, detail) : null,
		    (0, import_react.createElement)("button", { className: "rc-btn rc-btn-pri", style: { alignSelf: "flex-start" }, onClick: onRetry }, "\u27F3 \u91CD\u65B0\u68C0\u6D4B")
		  );
		}
		function LoadingState() {
		  return (0, import_react.createElement)(
		    "div",
		    { className: "rc-loading" },
		    (0, import_react.createElement)("div", { className: "rc-loading-t" }, (0, import_react.createElement)("span", { className: "rc-spin" }), "\u641C\u7D22\u4E2D\u2026"),
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-skel" },
		      (0, import_react.createElement)("div", { className: "rc-skel-row" }),
		      (0, import_react.createElement)("div", { className: "rc-skel-row" }),
		      (0, import_react.createElement)("div", { className: "rc-skel-row" })
		    )
		  );
		}
		function AnyfindPanel() {
		  const [health, setHealth] = (0, import_react.useState)(null);
		  const [downDetail, setDownDetail] = (0, import_react.useState)(null);
		  const [query, setQuery] = (0, import_react.useState)("");
		  const [activeKind, setActiveKind] = (0, import_react.useState)("");
		  const [resp, setResp] = (0, import_react.useState)(null);
		  const [error, setError] = (0, import_react.useState)(null);
		  const [busy, setBusy] = (0, import_react.useState)(false);
		  const [synOpen, setSynOpen] = (0, import_react.useState)(false);
		  const [sel, setSel] = (0, import_react.useState)(-1);
		  const [cmds, setCmds] = (0, import_react.useState)({});
		  const [resumeBusy, setResumeBusy] = (0, import_react.useState)(null);
		  const [toast, setToast] = (0, import_react.useState)("");
		  const seq = (0, import_react.useRef)(0);
		  const inputRef = (0, import_react.useRef)(null);
		  const hlRef = (0, import_react.useRef)(null);
		  const checkHealth = () => {
		    void getHealth().then(
		      (h) => {
		        setHealth(h);
		        setDownDetail(null);
		      },
		      (e) => {
		        setHealth(null);
		        setDownDetail(e instanceof Error ? e.message : String(e));
		      }
		    );
		  };
		  (0, import_react.useEffect)(checkHealth, []);
		  (0, import_react.useEffect)(() => {
		    if (toast === "") return;
		    const t = setTimeout(() => {
		      setToast("");
		    }, 2200);
		    return () => {
		      clearTimeout(t);
		    };
		  }, [toast]);
		  const search = () => {
		    const q = query.trim();
		    if (q === "" || busy) return;
		    const my = ++seq.current;
		    setBusy(true);
		    setError(null);
		    setSynOpen(false);
		    void getSearch(q).then(
		      (r) => {
		        if (seq.current === my) {
		          setResp(r);
		          setBusy(false);
		          setSel(0);
		        }
		      },
		      (e) => {
		        if (seq.current !== my) return;
		        setBusy(false);
		        if (e instanceof CoreDown) {
		          setHealth(null);
		          setDownDetail(e.message);
		          return;
		        }
		        setError(e instanceof Error ? e.message : String(e));
		      }
		    );
		  };
		  const insertSyntax = (ins) => {
		    const input = inputRef.current;
		    const at = input !== null && document.activeElement === input ? input.selectionStart : null;
		    setQuery((prev) => {
		      const pos = at ?? prev.length;
		      const glue = pos > 0 && prev[pos - 1] !== " " ? " " : "";
		      return `${prev.slice(0, pos)}${glue}${ins}${prev.slice(pos)}`;
		    });
		    requestAnimationFrame(() => {
		      inputRef.current?.focus();
		      if (inputRef.current !== null && at !== null) {
		        const p = at + ins.length;
		        inputRef.current.setSelectionRange(p, p);
		      }
		    });
		  };
		  const copyText = (text, tip) => {
		    void navigator.clipboard.writeText(text).then(
		      () => {
		        setToast(tip);
		      },
		      () => {
		        setToast("\u590D\u5236\u5931\u8D25\uFF08\u526A\u8D34\u677F\u6743\u9650\uFF09\uFF0C\u8BF7\u624B\u52A8\u590D\u5236");
		      }
		    );
		  };
		  const primaryAction = (key, hit) => {
		    if (hit.session_id !== null && hit.agent !== null) {
		      setResumeBusy(key);
		      void postResume(hit).then(
		        (command) => {
		          setResumeBusy(null);
		          setCmds((prev) => ({ ...prev, [key]: command }));
		          copyText(command, "\u5DF2\u590D\u5236 resume \u547D\u4EE4");
		        },
		        (e) => {
		          setResumeBusy(null);
		          if (e instanceof CoreDown) {
		            setHealth(null);
		            setDownDetail("\u5185\u6838\u8FDE\u63A5\u4E2D\u65AD");
		            return;
		          }
		          setError(e instanceof Error ? e.message : String(e));
		        }
		      );
		      return;
		    }
		    if (hit.path !== null) copyText(hit.path, "\u5DF2\u590D\u5236\u8DEF\u5F84");
		  };
		  const counts = {};
		  if (resp !== null) {
		    let sum = 0;
		    let anyTrunc = false;
		    for (const d of SEG_DEFS) {
		      if (d.k === "") continue;
		      const n = (resp.groups[d.k] ?? []).length;
		      const trunc = resp.truncated?.[d.k] === true;
		      counts[d.k] = n > 0 ? `${n}${trunc ? "+" : ""}` : "";
		      sum += n;
		      if (trunc) anyTrunc = true;
		    }
		    counts[""] = sum > 0 ? `${sum}${anyTrunc ? "+" : ""}` : "";
		  } else if (health?.index !== void 0) {
		    const idx = health.index;
		    counts[""] = fmtCount((idx.files ?? 0) + (idx.sessions ?? 0) + (idx.skills ?? 0) + (idx.memories ?? 0));
		    counts.file = fmtCount(idx.files);
		    counts.session = fmtCount(idx.sessions);
		    counts.skill = fmtCount(idx.skills);
		    counts.memory = fmtCount(idx.memories);
		  }
		  const visibleGroups = resp === null ? [] : (activeKind === "" ? KINDS : [activeKind]).map((k) => ({ kind: k, hits: resp.groups[k] ?? [] })).filter((g) => activeKind !== "" || g.hits.length > 0);
		  const total = resp === null ? 0 : KINDS.reduce((n, k) => n + (resp.groups[k] ?? []).length, 0);
		  const flatHits = visibleGroups.flatMap((g) => g.hits.map((h, i) => ({ key: `${g.kind}-${i}`, hit: h })));
		  const selClamped = sel >= 0 && sel < flatHits.length ? sel : -1;
		  const onInputKeyDown = (e) => {
		    if (e.key === "Enter") {
		      e.preventDefault();
		      if (selClamped >= 0) primaryAction(flatHits[selClamped].key, flatHits[selClamped].hit);
		      else search();
		      return;
		    }
		    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
		      if (flatHits.length === 0) return;
		      e.preventDefault();
		      const delta = e.key === "ArrowDown" ? 1 : -1;
		      setSel((prev) => {
		        const cur = prev >= 0 && prev < flatHits.length ? prev : delta === 1 ? -1 : 0;
		        return Math.min(Math.max(cur + delta, 0), flatHits.length - 1);
		      });
		    }
		  };
		  const tint = tintQuery(query);
		  return (0, import_react.createElement)(
		    "div",
		    { className: "rc-panel" },
		    (0, import_react.createElement)(StyleTag),
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-head" },
		      (0, import_react.createElement)(
		        "div",
		        { className: "rc-input-wrap" },
		        (0, import_react.createElement)(
		          "div",
		          { className: "rc-input-hl", ref: hlRef, "aria-hidden": true },
		          ...tint.map((t, i) => t.cls === null ? (0, import_react.createElement)("span", { key: i }, t.text) : (0, import_react.createElement)("span", { key: i, className: t.cls }, t.text)),
		          // 尾部零宽占位保证末尾空格可见宽度同步
		          (0, import_react.createElement)("span", null, "\u200B")
		        ),
		        (0, import_react.createElement)("input", {
		          className: "rc-input",
		          ref: inputRef,
		          placeholder: "\u641C\u5168\u90E8\u5386\u53F2\uFF1A\u6587\u4EF6 / \u4F1A\u8BDD / \u6280\u80FD / \u8BB0\u5FC6\u2026",
		          value: query,
		          spellCheck: false,
		          onChange: (e) => {
		            setQuery(e.target.value);
		          },
		          onScroll: () => {
		            if (inputRef.current !== null && hlRef.current !== null) {
		              hlRef.current.scrollLeft = inputRef.current.scrollLeft;
		            }
		          },
		          onKeyDown: onInputKeyDown
		        }),
		        (0, import_react.createElement)("button", {
		          className: "rc-syn-btn",
		          title: "\u8FC7\u6EE4\u8BED\u6CD5",
		          onClick: () => {
		            setSynOpen((v) => !v);
		          }
		        }, "?")
		      ),
		      (0, import_react.createElement)("button", { className: "rc-btn rc-btn-pri", onClick: search, disabled: busy || query.trim() === "" }, "\u641C\u7D22"),
		      synOpen ? (0, import_react.createElement)(
		        "div",
		        { className: "rc-syn" },
		        (0, import_react.createElement)("div", { className: "rc-syn-t" }, "\u8FC7\u6EE4\u8BED\u6CD5\uFF08\u70B9\u51FB\u63D2\u5165\uFF09"),
		        ...SYNTAX_ROWS.map((r) => (0, import_react.createElement)(
		          "div",
		          {
		            key: r.ins,
		            className: "rc-syn-row",
		            onClick: () => {
		              insertSyntax(r.ins);
		            }
		          },
		          (0, import_react.createElement)("code", null, r.ins),
		          (0, import_react.createElement)("span", null, r.desc)
		        )),
		        (0, import_react.createElement)("div", { className: "rc-syn-ex" }, "\u7EC4\u5408\u793A\u4F8B\uFF1A", (0, import_react.createElement)("code", null, 'kind:session agent:codex "nginx \u53CD\u4EE3" -test'))
		      ) : null
		    ),
		    (0, import_react.createElement)(SegControl, { active: activeKind, counts, onChange: (k) => {
		      setActiveKind(k);
		      setSel(0);
		    } }),
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-body" },
		      health === null ? (0, import_react.createElement)(DownGuide, { detail: downDetail ?? "", onRetry: checkHealth }) : null,
		      busy ? (0, import_react.createElement)(LoadingState) : null,
		      !busy && health !== null && resp === null ? (0, import_react.createElement)(
		        "div",
		        { className: "rc-hit-meta" },
		        (0, import_react.createElement)("span", { className: "rc-meta-pill" }, `\u25CF \u5185\u6838\u5C31\u7EEA${health.version !== void 0 ? ` v${health.version}` : ""}`),
		        health.index !== void 0 ? (0, import_react.createElement)("span", null, `\u7D22\u5F15\uFF1A\u6587\u4EF6 ${health.index.files ?? 0} \xB7 \u4F1A\u8BDD ${health.index.sessions ?? 0} \xB7 \u6280\u80FD ${health.index.skills ?? 0} \xB7 \u8BB0\u5FC6 ${health.index.memories ?? 0}`) : null,
		        (0, import_react.createElement)("span", { style: { flex: 1 } }),
		        (0, import_react.createElement)("span", { className: "rc-kbd" }, "\u2191\u2193 \u5BFC\u822A \xB7 Enter \u6267\u884C \xB7 Esc \u5173\u95ED")
		      ) : null,
		      error !== null ? (0, import_react.createElement)("div", { className: "rc-banner rc-banner-err" }, error) : null,
		      !busy && resp !== null && total === 0 ? (0, import_react.createElement)("div", { className: "rc-banner rc-banner-info" }, `\u300C${resp.q}\u300D\u6CA1\u6709\u547D\u4E2D\u2014\u2014\u6362\u4E2A\u5173\u952E\u8BCD\uFF0C\u6216\u53BB\u6389\u8FC7\u6EE4\u6761\u4EF6\u3002`) : null,
		      !busy && resp !== null && total > 0 && activeKind !== "" && visibleGroups.every((g) => g.hits.length === 0) ? (0, import_react.createElement)("div", { className: "rc-banner rc-banner-info" }, `\u300C${resp.q}\u300D\u5728\u300C${KIND_LABEL[activeKind]}\u300D\u7EC4\u6CA1\u6709\u547D\u4E2D\uFF08\u5168\u90E8 ${total} \u6761\u5728\u5176\u4ED6\u7EC4\uFF09\u3002`) : null,
		      !busy && resp !== null && total > 0 && activeKind === "" ? (0, import_react.createElement)("div", { className: "rc-hit-meta" }, `\u547D\u4E2D ${total} \u6761${resp.took_ms !== void 0 ? ` \xB7 ${resp.took_ms}ms` : ""}`) : null,
		      ...!busy ? visibleGroups.map((g) => (0, import_react.createElement)(
		        "div",
		        { key: g.kind, className: "rc-group" },
		        (0, import_react.createElement)("div", { className: "rc-group-t" }, `${KIND_LABEL[g.kind]}\uFF08${g.hits.length}\uFF09`),
		        ...g.hits.map((h, i) => {
		          const key = `${g.kind}-${i}`;
		          const flatIdx = flatHits.findIndex((f) => f.key === key);
		          return (0, import_react.createElement)(HitRow, {
		            key,
		            hit: h,
		            focused: flatIdx === selClamped,
		            cmd: cmds[key] ?? null,
		            busyKey: resumeBusy === key,
		            onResume: (hh) => {
		              primaryAction(key, hh);
		            },
		            onCopy: copyText,
		            onDown: () => {
		              setHealth(null);
		              setDownDetail("\u5185\u6838\u8FDE\u63A5\u4E2D\u65AD");
		            }
		          });
		        })
		      )) : []
		    ),
		    toast !== "" ? (0, import_react.createElement)("div", { className: "rc-toast" }, toast) : null
		  );
		}
		function AnyfindOverlay() {
		  const open = (0, import_react.useSyncExternalStore)(overlayStore.subscribe, () => overlayStore.open);
		  (0, import_react.useEffect)(() => {
		    const onKey = (e) => {
		      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
		        if (e.isComposing) return;
		        e.preventDefault();
		        overlayStore.toggle();
		      }
		    };
		    window.addEventListener("keydown", onKey);
		    return () => {
		      window.removeEventListener("keydown", onKey);
		    };
		  }, []);
		  (0, import_react.useEffect)(() => {
		    if (!open) return;
		    const t = setTimeout(() => {
		      document.querySelector(".rc-modal .rc-input")?.focus();
		    }, 60);
		    const onKey = (e) => {
		      if (e.key === "Escape") overlayStore.close();
		    };
		    window.addEventListener("keydown", onKey);
		    return () => {
		      clearTimeout(t);
		      window.removeEventListener("keydown", onKey);
		    };
		  }, [open]);
		  if (!open) return null;
		  return (0, import_react.createElement)(
		    "div",
		    {
		      className: "rc-mask",
		      onClick: (e) => {
		        if (e.target === e.currentTarget) overlayStore.close();
		      }
		    },
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-modal", role: "dialog", "aria-label": "\u7EDF\u4E00\u641C\u7D22" },
		      (0, import_react.createElement)(AnyfindPanel)
		    )
		  );
		}
		var bootAt = 0;
		function AnyfindTabPlaceholder(props) {
		  (0, import_react.useEffect)(() => {
		    const isRestore = Date.now() - bootAt <= 5e3;
		    if (!isRestore) overlayStore.show();
		    const tabId = props.tab?.id;
		    if (tabId !== void 0 && betterSidebarSvc !== null && typeof betterSidebarSvc.closeTab === "function") {
		      betterSidebarSvc.closeTab(tabId, props.scope);
		      const store = props.store;
		      const st = store?.getSnapshot().state;
		      if (store !== void 0 && st?.bottomOpen === true && countRemainingTabs(st.bottomSplits) === 0) {
		        store.reduce((s) => ({ ...s, bottomOpen: false }));
		      }
		    }
		  }, []);
		  return (0, import_react.createElement)(
		    "div",
		    { className: "rc-panel" },
		    (0, import_react.createElement)(StyleTag),
		    (0, import_react.createElement)(
		      "div",
		      { className: "rc-ph" },
		      (0, import_react.createElement)(AnyfindMark, { size: 28 }),
		      (0, import_react.createElement)("div", { className: "rc-ph-t" }, "\u641C\u7D22\u4EE5\u6D6E\u5C42\u6253\u5F00"),
		      (0, import_react.createElement)("div", null, "\u6309 ", (0, import_react.createElement)("kbd", null, "Ctrl+K"), " \u968F\u65F6\u547C\u51FA / Esc \u5173\u95ED"),
		      (0, import_react.createElement)("button", { className: "rc-btn rc-btn-pri", onClick: () => {
		        overlayStore.show();
		      } }, "\u91CD\u65B0\u6253\u5F00\u641C\u7D22")
		    )
		  );
		}
		function AnyfindTabTitle() {
		  return (0, import_react.createElement)("span", {
		    style: { display: "inline-flex", alignItems: "center", gap: 6, color: "var(--dsw-alias-label-primary, inherit)" }
		  }, (0, import_react.createElement)(AnyfindMark, { size: 14 }), "\u641C\u7D22");
		}
		function AnyfindGuideIcon(props) {
		  return (0, import_react.createElement)(AnyfindMark, { size: props?.size ?? 22 });
		}
		function apply(ctx) {
		  const slots = ctx.slots;
		  if (slots === null || typeof slots !== "object" || typeof slots.inject !== "function" || typeof slots.register !== "function") {
		    console.warn("[dsh-anyfind] \u5BBF\u4E3B\u672A\u63D0\u4F9B\u53EF\u7528\u7684 slots \u670D\u52A1\uFF08\u9700\u8981 @deepseek-ai/dsh-client-ui-renderer\uFF09\uFF0C\u641C\u7D22\u6D6E\u5C42\u8DF3\u8FC7\u6302\u8F7D");
		    return;
		  }
		  const dyn = ctx.slots;
		  bootAt = Date.now();
		  ctx.slots.inject("shell.overlay", () => ctx.slots.register(
		    { name: "shell.overlay", id: "anyfind-spotlight", order: 48 },
		    () => (0, import_react.createElement)(AnyfindOverlay)
		  ));
		  const registerDirectTab = (tabs) => {
		    const disposeType = tabs.register({
		      id: "dsh-anyfind",
		      kind: "anyfind",
		      keepMounted: false,
		      title: () => "\u641C\u7D22",
		      guide: [{
		        id: "anyfind",
		        order: 62,
		        title: () => "\u641C\u7D22",
		        description: () => "\u7EDF\u4E00\u641C\u7D22\uFF08\u6D6E\u5C42\uFF09\uFF1A\u6587\u4EF6 / AI \u4F1A\u8BDD / \u6280\u80FD / \u8BB0\u5FC6\uFF08Ctrl+K\uFF09",
		        icon: AnyfindGuideIcon
		      }]
		    });
		    const disposeBody = dyn.inject("sidebar.right.pane.tab", () => dyn.register(
		      { name: "sidebar.right.pane.tab", key: "dsh-anyfind", registrant: "dsh-anyfind" },
		      (props) => (0, import_react.createElement)(AnyfindTabPlaceholder, props)
		    ));
		    const disposeTitle = dyn.inject("sidebar.right.pane.tab.title", () => dyn.register(
		      { name: "sidebar.right.pane.tab.title", key: "dsh-anyfind", registrant: "dsh-anyfind" },
		      AnyfindTabTitle
		    ));
		    return () => {
		      disposeBody();
		      disposeTitle();
		      disposeType();
		    };
		  };
		  let tabsRegistry = null;
		  const syncDirectTab = () => {
		    if (betterSidebarSvc !== null) {
		      if (disposeDirectTab !== null) {
		        disposeDirectTab();
		        disposeDirectTab = null;
		      }
		      return;
		    }
		    if (tabsRegistry !== null && disposeDirectTab === null) {
		      disposeDirectTab = registerDirectTab(tabsRegistry);
		    }
		  };
		  ctx.inject(["sidebarRightTabs"], (sc) => {
		    tabsRegistry = sc.get("sidebarRightTabs") ?? null;
		    syncDirectTab();
		    return () => {
		      tabsRegistry = null;
		      if (disposeDirectTab !== null) {
		        disposeDirectTab();
		        disposeDirectTab = null;
		      }
		      syncDirectTab();
		    };
		  });
		  ctx.inject(["betterSidebar"], (sc) => {
		    betterSidebarSvc = sc.get("betterSidebar") ?? null;
		    let disposeBs = null;
		    if (betterSidebarSvc !== null) {
		      disposeBs = betterSidebarSvc.registerTab({
		        id: "anyfind",
		        title: () => "\u641C\u7D22",
		        description: () => "\u7EDF\u4E00\u641C\u7D22\uFF08\u6D6E\u5C42\uFF09\uFF1A\u6587\u4EF6 / AI \u4F1A\u8BDD / \u6280\u80FD / \u8BB0\u5FC6\uFF08Ctrl+K\uFF09",
		        icon: (size) => AnyfindGuideIcon({ size }),
		        order: 62,
		        single: true,
		        component: (props) => (0, import_react.createElement)(AnyfindTabPlaceholder, props)
		      });
		    }
		    syncDirectTab();
		    return () => {
		      if (disposeBs !== null) disposeBs();
		      betterSidebarSvc = null;
		      syncDirectTab();
		    };
		  });
		}
		
		return module.exports;
	}
});
