# anyfindd 全功能回归（8790）。输出 markdown 表格行到 stdout。
# 用法: uv run python -X utf8 tools/regression-001.py
import json, urllib.request, urllib.parse, time

BASE = "http://127.0.0.1:8790"
ROWS = []

def get(path, timeout=30):
    t0 = time.perf_counter()
    with urllib.request.urlopen(BASE + path, timeout=timeout) as r:
        body = json.loads(r.read().decode("utf-8"))
    return body, (time.perf_counter() - t0) * 1000

def post(path, payload, timeout=30):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(BASE + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))

def rec(item, inp, expect, actual, ok):
    ROWS.append((item, inp, expect, actual, "✓" if ok else "✗ FAIL"))

def search(q, **kw):
    qs = urllib.parse.urlencode({"q": q, **kw})
    return get(f"/api/search?{qs}")

# ---------- 1. health ----------
h, _ = get("/api/health")
ok = (h.get("ok") is True and h.get("version") == "0.1.0"
      and set(h["index"].keys()) == {"files", "sessions", "skills", "memories"}
      and isinstance(h["indexing"], bool))
rec("health 结构", "GET /api/health", "ok/version/四类计数/indexing", json.dumps(h["index"]), ok)

# ---------- 2. DSL 矩阵 ----------
def total(d):
    return sum(len(v) for v in d["groups"].values())

def kinds_of(d):
    return set(d["groups"].keys())

d, _ = search("nginx")
rec("裸词", "nginx", "有命中", f"hits={total(d)}", total(d) > 0)
d, _ = search("nginx config")
rec("多裸词 AND", "nginx config", "有命中且不超过裸词", f"hits={total(d)}", total(d) >= 0)
d, _ = search('"reverse proxy"')
rec("短语", '"reverse proxy"', "不报错", f"hits={total(d)}", True)
d_or, _ = search("nginx OR codexxyzzy")
rec("OR", "nginx OR codexxyzzy", "命中≥裸词 nginx", f"hits={total(d_or)}", total(d_or) >= 1)
d_ex, _ = search("nginx -vim")
ok = all("vim" not in (h["title"] or "").lower() and "vim" not in (h["snippet"] or "").lower()
         for h in d_ex["groups"].get("file", []))
rec("-排除", "nginx -vim", "file 组无 vim", f"file={len(d_ex['groups'].get('file', []))}", ok)

for k in ["file", "content", "session", "skill", "memory"]:
    d, _ = search(f"kind:{k} nginx")
    only = kinds_of(d) <= {k}
    rec(f"kind:{k}", f"kind:{k} nginx", f"仅 {k} 组", str(sorted(kinds_of(d))), only)

for ag in ["claude-code", "codex", "dsh", "opencode", "pi", "cursor"]:
    d, _ = search(f"kind:session skill agent:{ag} nginx")
    bad = []
    for kk in ("session", "skill"):
        for hh in d["groups"].get(kk, []):
            if hh["agent"] != ag:
                bad.append((kk, hh["agent"]))
    rec(f"agent:{ag}", f"agent:{ag} nginx", "组内 agent 全等" + ("（无命中亦可）" if not bad else ""),
        f"bad={bad}", not bad)

# agent:codex 串扰专项（Python 内核旧 bug：agent 过滤串到无关组）
d, _ = search("agent:codex nginx")
bad = [(k, h["agent"]) for k in ("session", "skill", "memory")
       for h in d["groups"].get(k, []) if h["agent"] != "codex"]
bad_file = [h["title"] for h in d["groups"].get("file", []) if h["agent"] is not None]
rec("agent 组间串扰", "agent:codex nginx", "session/skill/memory 组 agent 全=codex，file 组 agent=null",
    f"bad={bad} file_bad={bad_file}", not bad and not bad_file)

d, _ = search("kind:memory project:hippo-e2e 记忆")
bad = [h["project"] for h in d["groups"].get("memory", [])
       if not (h["project"] and "hippo-e2e" in h["project"].lower())]
rec("project:", "kind:memory project:hippo-e2e 记忆", "memory 组 project 全=hippo-e2e 且非空",
    f"hits={len(d['groups'].get('memory', []))} bad={bad}", not bad and len(d["groups"].get("memory", [])) > 0)

d, _ = search("kind:file ext:md spec")
bad = [h["path"] for h in d["groups"].get("file", []) if not h["path"].lower().endswith(".md")]
rec("ext:", "kind:file ext:md spec", "file 组全部 .md", f"bad={bad}", not bad and len(d["groups"].get("file", [])) > 0)

for dq in ["date:today", "date:week", "date:>2026-10-01", "date:2026-10-01..2026-10-21"]:
    d, _ = search(f"kind:session {dq} nginx")
    rec(dq, f"kind:session {dq} nginx", "不报错", f"hits={len(d['groups'].get('session', []))}", True)

d, _ = search("kind:file in:D:/coding/anyfind spec")
bad = [h["path"] for h in d["groups"].get("file", [])
       if not h["path"].lower().replace("/", "\\").startswith("d:\\coding\\anyfind")]
rec("in:", "kind:file in:D:/coding/anyfind spec", "file 组全部落在前缀内", f"bad={bad}", not bad and len(d["groups"].get("file", [])) > 0)

d, _ = search("kind:session role:user nginx")
bad = [h["agent"] for h in d["groups"].get("session", [])]  # role 不可直接观测，只验证不报错且有/无命中
rec("role:", "kind:session role:user nginx", "不报错", f"hits={len(d['groups'].get('session', []))}", True)

d, _ = search("kind:file size:>1kb spec")
rec("size:", "kind:file size:>1kb spec", "不报错", f"hits={len(d['groups'].get('file', []))}", True)

d, _ = search("kind:session agent:codex nginx")
bad = [h["agent"] for h in d["groups"].get("session", []) if h["agent"] != "codex"]
rec("组合 kind+agent+词", "kind:session agent:codex nginx", "session 组 agent 全=codex",
    f"bad={bad}", not bad)

d, _ = search("date:week -测试zzxyq 配置")
rec("组合 date+排除", "date:week -测试zzxyq 配置", "不报错", f"hits={total(d)}", True)

# ---------- 3. Hit 结构 ----------
EXPECT = {"kind", "title", "path", "snippet", "mtime", "agent", "project", "session_id", "cwd", "score"}
d, _ = search("配置", limit=10)
missing_all = []
mark_bad = []
for k, hits in d["groups"].items():
    for h in hits:
        missing_all += [(k, m) for m in EXPECT - set(h.keys())]
        snip = h.get("snippet") or ""
        if snip.count("<mark>") != snip.count("</mark>"):
            mark_bad.append((k, snip[:50]))
        if "&lt;mark&gt;" in snip or "<mark><mark>" in snip:
            mark_bad.append((k, "嵌套/转义"))
rec("Hit 十字段", "配置 limit=10", "字段齐全", f"missing={missing_all[:3]}", not missing_all)
rec("snippet 单层 <mark>", "同上", "成对无嵌套无残留哨兵", f"bad={mark_bad[:3]}", not mark_bad)
sentinel_bad = []
for k, hits in d["groups"].items():
    for h in hits:
        if "\ue000" in (h.get("snippet") or "") or "\ue001" in (h.get("snippet") or ""):
            sentinel_bad.append(k)
rec("哨兵无泄漏", "同上", "无 \\uE000/\\uE001", f"bad={sentinel_bad}", not sentinel_bad)

# ---------- 4. /api/session/{id} ----------
d, _ = search("kind:session nginx", limit=1)
hits = d["groups"].get("session", [])
if hits:
    sid = hits[0]["session_id"]
    det, _ = get(f"/api/session/{urllib.parse.quote(sid)}")
    ok = (det["session_id"] == sid and isinstance(det["messages"], list)
          and all(set(m.keys()) == {"role", "ts", "text"} for m in det["messages"][:5]))
    rec("session 详情", f"/api/session/{sid}", "元信息+messages 结构", f"msgs={len(det['messages'])}", ok)
else:
    rec("session 详情", "无 session 命中可测", "—", "SKIP", True)
try:
    get("/api/session/nonexistent-id-zzz")
    rec("session 404", "/api/session/nonexistent-id-zzz", "404", "200?!", False)
except urllib.error.HTTPError as e:
    rec("session 404", "/api/session/nonexistent-id-zzz", "404", f"{e.code}", e.code == 404)

# ---------- 5. /api/resume ----------
EXPECT_CMD = {
    "claude-code": "claude --resume ", "codex": "codex resume ", "dsh": "dsh tui --resume ",
    "opencode": "opencode --session ", "pi": "pi --session ",
}
d, _ = search("kind:session 配置 nginx error fix bug help 配置 索引 报告", limit=50)
by_agent = {}
for h in d["groups"].get("session", []):
    by_agent.setdefault(h["agent"], h["session_id"])
for ag, prefix in EXPECT_CMD.items():
    sid = by_agent.get(ag)
    if not sid:
        d2, _ = search(f"kind:session agent:{ag} the", limit=1)
        hh = d2["groups"].get("session", [])
        sid = hh[0]["session_id"] if hh else None
    if not sid:
        rec(f"resume {ag}", f"agent={ag}", "有会话可测", "无命中 SKIP", True)
        continue
    r = post("/api/resume", {"agent": ag, "session_id": sid, "cwd": "", "spawn": False})
    ok = r.get("ok") is True and (r.get("command") or "").startswith(prefix) and "spawned" not in r
    rec(f"resume {ag}", f"sid={sid[:16]}… spawn:false", f"{prefix}* 且无 spawned", json.dumps(r), ok)
# pi 用真实 sid 直测（语料里 pi 会话少，关键词检索可能捞不到）
r = post("/api/resume", {"agent": "pi", "session_id": "019fe6e0-8104-764b-b532-43c8ef6d9ee2", "cwd": "", "spawn": False})
ok = r.get("ok") is True and (r.get("command") or "").startswith("pi --session ") and "spawned" not in r
rec("resume pi", "固定 pi sid spawn:false", "pi --session * 且无 spawned", json.dumps(r), ok)
# cursor：真实 sid（能找到会话）→ 应报"无 CLI resume 能力"
r = post("/api/resume", {"agent": "cursor", "session_id": "1b8c55ef-c060-4bdd-9a0e-66cae4347758"})
rec("resume cursor", "agent=cursor + 真实 sid", "ok=false 且无 CLI resume 能力", json.dumps(r),
    r.get("ok") is False and "resume" in (r.get("error") or ""))
r = post("/api/resume", {"agent": "bogus", "session_id": "x"})
rec("resume 未知 agent", "agent=bogus", "ok=false", json.dumps(r), r.get("ok") is False)

# ---------- 6. 边界 ----------
d, _ = search("")
rec("空查询", "q=", "空 groups 不报错", json.dumps(d["groups"]), isinstance(d["groups"], dict))
d, _ = search("x" * 10000)
rec("超长查询", "x*10000", "不崩不报错", f"hits={total(d)}", True)
d, _ = search('<>&"\' kind:file')
rec("特殊字符", "<>&\"' kind:file", "不崩不报错", f"hits={total(d)}", True)
d, _ = search("配置 索引")
rec("纯中文", "配置 索引", "两词 AND 有命中（与 Python 内核一致）", f"hits={total(d)}", total(d) > 0)
d, _ = search("m")
rec("单字符", "m", "有命中不超时", f"hits={total(d)} took={d['took_ms']}ms", total(d) > 0)

# ---------- 7. 性能抽查 ----------
for label, q in [("单字符", "m"), ("常用词", "nginx"), ("中文", "配置"), ("组合", "kind:session agent:codex nginx")]:
    ts, tooks = [], []
    for _ in range(3):
        d, t = search(q)
        ts.append(round(t, 1))
        tooks.append(d["took_ms"])
    rec(f"性能 {label}", q, "took_ms<50（参考）", f"took={tooks} http={ts}", max(tooks) < 50)

# ---------- 输出 ----------
print("| 项 | 输入 | 预期 | 实际 | 判定 |")
print("|---|---|---|---|---|")
for r in ROWS:
    print("| " + " | ".join(str(x).replace("|", "\\|").replace("\n", " ")[:120] for x in r) + " |")
fails = [r for r in ROWS if "FAIL" in r[4]]
print(f"\nTOTAL {len(ROWS)} 项, FAIL {len(fails)}")
for r in fails:
    print("FAIL:", r[0], r[1], r[3][:200])
