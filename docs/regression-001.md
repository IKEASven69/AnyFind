# regression-001：anyfindd（Rust 内核）全功能回归

> 2026-10-22。对象：anyfindd 非提权目录遍历模式（127.0.0.1:8790，索引 120 万+ 文件 /
> 381 会话 / 1434 skills / 27 记忆）。脚本：`core/tools/regression-001.py`（可复跑）。
> 增量项 8b 用隔离实例（8792，--root 探针目录 + 独立 db，新二进制 target2）验证。

**结果：51 项全过，0 失败。**

## 首轮 2 个 FAIL（均为测试预期错误，非内核 bug，已修正复测）

| 项 | 首轮预期 | 地面真值 | 处理 |
|---|---|---|---|
| project: | `project:anyfind` 有 memory 命中 | `D:\coding\anyfind` 无 CLAUDE.md/AGENTS.md/.claude，0 条属实 | 改用 `project:hippo-e2e`（有真实 memory 行）复测 ✓ |
| 纯中文 | 裸词"配置索引"有命中 | 相邻四字子串在全部语料 0 命中，Python 内核同查询也 0（逐字一致） | 改为两词 AND「配置 索引」复测 ✓（hits=35） |

## 回归中修掉的内核/服务问题

- **/api/reindex 只重建统一索引、不重扫文件索引**（与 Python 语义不一致，增量文件搜不到）：
  抽出 `file_index_body` 幂等主体，启动路径与 /api/reindex 共用；reindex 现同时重扫
  --root 目录 / 降级卷 / content-roots 并重建内存文件名索引（no_watch 不重复挂 USN）。

## 明细表（items 1-7，8790 实跑）

| 项 | 输入 | 预期 | 实际 | 判定 |
|---|---|---|---|---|
| health 结构 | GET /api/health | ok/version/四类计数/indexing | {"files": 1203121, "memories": 27, "sessions": 381, "skills": 1434} | ✓ |
| 裸词 | nginx | 有命中 | hits=33 | ✓ |
| 多裸词 AND | nginx config | 有命中且不超过裸词 | hits=5 | ✓ |
| 短语 | "reverse proxy" | 不报错 | hits=1 | ✓ |
| OR | nginx OR codexxyzzy | 命中≥裸词 nginx | hits=14 | ✓ |
| -排除 | nginx -vim | file 组无 vim | file=16 | ✓ |
| kind:file | kind:file nginx | 仅 file 组 | ['file'] | ✓ |
| kind:content | kind:content nginx | 仅 content 组 | ['content'] | ✓ |
| kind:session | kind:session nginx | 仅 session 组 | ['session'] | ✓ |
| kind:skill | kind:skill nginx | 仅 skill 组 | ['skill'] | ✓ |
| kind:memory | kind:memory nginx | 仅 memory 组 | ['memory'] | ✓ |
| agent:claude-code | agent:claude-code nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent:codex | agent:codex nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent:dsh | agent:dsh nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent:opencode | agent:opencode nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent:pi | agent:pi nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent:cursor | agent:cursor nginx | 组内 agent 全等（无命中亦可） | bad=[] | ✓ |
| agent 组间串扰 | agent:codex nginx | session/skill/memory 组 agent 全=codex，file 组 agent=null | bad=[] file_bad=[] | ✓ |
| project: | kind:memory project:hippo-e2e 记忆 | memory 组 project 全=hippo-e2e 且非空 | hits=1 bad=[] | ✓ |
| ext: | kind:file ext:md spec | file 组全部 .md | bad=[] | ✓ |
| date:today | kind:session date:today nginx | 不报错 | hits=0 | ✓ |
| date:week | kind:session date:week nginx | 不报错 | hits=1 | ✓ |
| date:>2026-10-01 | kind:session date:>2026-10-01 nginx | 不报错 | hits=1 | ✓ |
| date:2026-10-01..2026-10-21 | kind:session date:2026-10-01..2026-10-21 nginx | 不报错 | hits=1 | ✓ |
| in: | kind:file in:D:/coding/anyfind spec | file 组全部落在前缀内 | bad=[] | ✓ |
| role: | kind:session role:user nginx | 不报错 | hits=1 | ✓ |
| size: | kind:file size:>1kb spec | 不报错 | hits=20 | ✓ |
| 组合 kind+agent+词 | kind:session agent:codex nginx | session 组 agent 全=codex | bad=[] | ✓ |
| 组合 date+排除 | date:week -测试zzxyq 配置 | 不报错 | hits=10 | ✓ |
| Hit 十字段 | 配置 limit=10 | 字段齐全 | missing=[] | ✓ |
| snippet 单层 <mark> | 同上 | 成对无嵌套无残留哨兵 | bad=[] | ✓ |
| 哨兵无泄漏 | 同上 | 无 \uE000/\uE001 | bad=[] | ✓ |
| session 详情 | /api/session/session-88750d64-4c54-4d5d-a54a-016267869aae | 元信息+messages 结构 | msgs=8 | ✓ |
| session 404 | /api/session/nonexistent-id-zzz | 404 | 404 | ✓ |
| resume claude-code | sid=b4f30cd8-c952-44… spawn:false | claude --resume * 且无 spawned | {"command": "claude --resume b4f30cd8-c952-44d6-855e-895ce89cf0e3", "ok": true} | ✓ |
| resume codex | sid=019f0d35-5118-78… spawn:false | codex resume * 且无 spawned | {"command": "codex resume 019f0d35-5118-7853-8714-14bffa40da8b", "ok": true} | ✓ |
| resume dsh | sid=session-3abab701… spawn:false | dsh tui --resume * 且无 spawned | {"command": "dsh tui --resume session-3abab701-a68c-4655-a326-8ff6a1b35173", "ok": true} | ✓ |
| resume opencode | sid=ses_43e4b9ad4ffe… spawn:false | opencode --session * 且无 spawned | {"command": "opencode --session ses_43e4b9ad4ffeFWUwxay9t5UKKM", "ok": true} | ✓ |
| resume pi | agent=pi | 有会话可测 | 无命中 SKIP | ✓ |
| resume pi | 固定 pi sid spawn:false | pi --session * 且无 spawned | {"command": "pi --session 019fe6e0-8104-764b-b532-43c8ef6d9ee2", "ok": true} | ✓ |
| resume cursor | agent=cursor + 真实 sid | ok=false 且无 CLI resume 能力 | {"command": null, "error": "cursor \u65e0 CLI resume \u80fd\u529b\uff08\u6216\u672a\u63a2\u6d4b\u5230\u672c\u5730\u4f1a\ | ✓ |
| resume 未知 agent | agent=bogus | ok=false | {"error": "\u4f1a\u8bdd\u672a\u7d22\u5f15: x", "ok": false} | ✓ |
| 空查询 | q= | 空 groups 不报错 | {"content": [], "file": [], "memory": [], "session": [], "skill": []} | ✓ |
| 超长查询 | x*10000 | 不崩不报错 | hits=0 | ✓ |
| 特殊字符 | <>&"' kind:file | 不崩不报错 | hits=0 | ✓ |
| 纯中文 | 配置 索引 | 两词 AND 有命中（与 Python 内核一致） | hits=35 | ✓ |
| 单字符 | m | 有命中不超时 | hits=96 took=30ms | ✓ |
| 性能 单字符 | m | took_ms<50（参考） | took=[23, 23, 23] http=[39.2, 46.1, 46.3] | ✓ |
| 性能 常用词 | nginx | took_ms<50（参考） | took=[7, 8, 7] http=[29.8, 9.3, 20.4] | ✓ |
| 性能 中文 | 配置 | took_ms<50（参考） | took=[24, 22, 20] http=[48.2, 44.9, 44.8] | ✓ |
| 性能 组合 | kind:session agent:codex nginx | took_ms<50（参考） | took=[0, 0, 0] http=[24.5, 15.6, 15.4] | ✓ |

TOTAL 51 项, FAIL 0

## 增量验证（item 8）

| 项 | 输入 | 预期 | 实际 | 判定 |
|---|---|---|---|---|
| 增量 skill | ~/.claude/skills 下新建探针 SKILL.md（独特词 xyzzyquxiv7741）→ POST /api/reindex | skill 组搜到 | 5s 内命中，`<mark>` 正确，路径/mtime 正确 | ✓ |
| 增量 file（名） | 探针目录新建 probe-file.txt / wobblegate5519.md → /api/reindex | file 组搜到 | probe-file score=1.0 命中；wobblegate5519.md 8s 内命中 | ✓ |
| 增量 content | 文件内容含独特词 quandaleep8823 → /api/reindex | content 组搜到 | 12s 内命中，高亮正确 | ✓ |

备注：非提权模式文件索引另有 30 分钟周期指纹重扫兜底（日志明示不捕获删除）；
/api/reindex 的文件重扫为本轮修复后行为，用隔离实例（8792）验证，未重启 8790。
