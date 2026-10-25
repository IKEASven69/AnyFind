"""field-only 与搜索→详情→resume 一致性真实索引回归。默认 8790。"""
import json
import os
import urllib.parse
import urllib.request

BASE = os.environ.get("ANYFIND_URL", "http://127.0.0.1:8790")


def get(path):
    with urllib.request.urlopen(BASE + path, timeout=60) as response:
        return json.load(response)


def post(path, body):
    request = urllib.request.Request(
        BASE + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def search(query, limit=5, offset=0):
    return get("/api/search?" + urllib.parse.urlencode(
        {"q": query, "limit": limit, "offset": offset}
    ))


queries = [
    ("kind:session agent:codex", "session"),
    ("kind:skill agent:claude-code", "skill"),
    ("kind:memory", "memory"),
    ("kind:file ext:md in:D:/coding/anyfind", "file"),
    ("kind:content ext:md in:D:/coding/anyfind", "content"),
]
for query, kind in queries:
    result = search(query)
    hits = result["groups"].get(kind, [])
    assert hits, f"纯字段查询无命中: {query}"
    assert all(hit["kind"] == kind for hit in hits), query
    print(f"PASS {query}: {len(hits)} 条, {result['took_ms']}ms")

empty = search("")
assert all(not hits for hits in empty["groups"].values())
print("PASS 完全空查询仍为空")

sessions = []
for offset in (0, 200):
    result = search("kind:session", limit=200, offset=offset)
    sessions.extend(result["groups"].get("session", []))
assert sessions, "没有可回归的会话"
failures = []
for hit in sessions:
    sid = hit["session_id"]
    agent = hit["agent"]
    try:
        get("/api/session/" + urllib.parse.quote(sid, safe=""))
        resumed = post("/api/resume", {
            "agent": agent,
            "session_id": sid,
            "cwd": hit.get("cwd") or "",
            "spawn": False,
        })
        if not resumed.get("ok") and agent != "cursor":
            failures.append((agent, sid, resumed.get("error")))
    except Exception as error:
        failures.append((agent, sid, str(error)))
assert not failures, f"搜索→详情→resume 失败: {failures[:10]}"
print(f"PASS {len(sessions)} 条会话全部可预览；非 Cursor 会话全部可生成 resume 命令")
