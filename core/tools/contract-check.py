import json, urllib.request, urllib.parse, time, sys

def get(port, path):
    t0 = time.perf_counter()
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=30) as r:
        body = json.loads(r.read().decode("utf-8"))
    return body, (time.perf_counter() - t0) * 1000

def hit_keys(h):
    return sorted(h.keys())

print("=== /api/health 结构 ===")
h, _ = get(8790, "/api/health")
assert h["ok"] is True and h["version"] == "0.1.0"
assert set(h["index"].keys()) == {"files", "sessions", "skills", "memories"}
assert isinstance(h["indexing"], bool)
print("ok:", h)

EXPECT_HIT = {"kind", "title", "path", "snippet", "mtime", "agent", "project", "session_id", "cwd", "score"}

print("\n=== /api/search 契约字段 + 与 Python 内核(8766) 对比 ===")
queries = ["nginx", "配置", "帮我看看", "kind:session agent:dsh 索引", "kind:skill 小红书",
           "agent:codex rust", "kind:memory AGENTS", "role:user opencode"]
for q in queries:
    qs = urllib.parse.urlencode({"q": q, "limit": 5})
    mine, t1 = get(8790, f"/api/search?{qs}")
    py, t2 = get(8766, f"/api/search?{qs}")
    assert mine["q"] == q and "groups" in mine and "took_ms" in mine
    for kind, hits in mine["groups"].items():
        for h in hits:
            missing = EXPECT_HIT - set(h.keys())
            assert not missing, f"{kind} hit 缺字段 {missing}"
    def sig(r, kind):
        return [(h.get("session_id") or h.get("path") or h.get("title")) for h in r["groups"].get(kind, [])]
    lines = [f"q={q!r}: mine took={mine['took_ms']}ms(http {t1:.0f}ms) py took={py['took_ms']}ms(http {t2:.0f}ms)"]
    for kind in ["session", "skill", "memory", "content"]:
        a, b = sig(mine, kind), sig(py, kind)
        overlap = len(set(map(str, a)) & set(map(str, b)))
        lines.append(f"   {kind:8s} mine={len(a)} py={len(b)} 重叠={overlap}")
    print("\n".join(lines))

print("\n=== /api/session/{id} ===")
mine, _ = get(8790, "/api/search?q=" + urllib.parse.quote("配置") + "&kinds=session&limit=1")
hits = mine["groups"].get("session", [])
if hits:
    sid = hits[0]["session_id"]
    agent = hits[0]["agent"]
    d, _ = get(8790, f"/api/session/{urllib.parse.quote(sid)}")
    assert d["session_id"] == sid and d["agent"] == agent
    assert isinstance(d["messages"], list) and d["messages"]
    m0 = d["messages"][0]
    assert set(m0.keys()) == {"role", "ts", "text"}
    print(f"ok: {agent}/{sid} messages={len(d['messages'])}")
else:
    print("SKIP: 无 session 命中")

print("\n=== POST /api/resume ===")
if hits:
    body = json.dumps({"agent": hits[0]["agent"], "session_id": hits[0]["session_id"], "cwd": hits[0].get("cwd") or ""}).encode()
    req = urllib.request.Request("http://127.0.0.1:8790/api/resume", data=body, headers={"Content-Type": "application/json"})
    r = json.loads(urllib.request.urlopen(req, timeout=10).read().decode())
    print("resume:", r)
    assert r["ok"] is True and r["command"]
    # 未知 agent
    body2 = json.dumps({"agent": "bogus", "session_id": "x"}).encode()
    req2 = urllib.request.Request("http://127.0.0.1:8790/api/resume", data=body2)
    r2 = json.loads(urllib.request.urlopen(req2, timeout=10).read().decode())
    print("bogus agent:", r2)

print("\n=== 延迟抽查（10 次中位） ===")
for q in ["m", "nginx", "配置", "kind:file spec"]:
    ts = []
    for _ in range(10):
        _, t = get(8790, f"/api/search?q={urllib.parse.quote(q)}")
        ts.append(t)
    ts.sort()
    print(f"{q!r}: http 中位 {ts[5]:.1f}ms")
print("\nALL PASS")
