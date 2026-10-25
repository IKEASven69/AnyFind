# bench-001：内容搜索引擎选型 — rusqlite FTS5 (trigram) vs Tantivy

> 2026-10-21。SPEC 硬要求：选型实验先做，真实语料，用数据定选型。
> 复现：`cd core && cargo run --release -p bench-engine`（bench-engine/src/main.rs）。
> 机器：Windows 11 x64, rustc 1.99, release 构建。

## 语料

| 语料 | 规模 | 来源 |
|---|---|---|
| 会话消息 | **15,752 条 / 10.3MB** | 六家适配器现采本机真实数据（claude-code 24 / codex 28 / dsh 157 / opencode 99 / pi 19 会话，共 327 会话，与 Python 内核索引数逐字一致） |
| 文件名 | **100,000 条** | filefind 运行中内核的全盘索引（%APPDATA%/filefind/index.db，全库 1,398,404 条）抽 10 万 |
| 文件名-全量对照 | 1,398,404 条 | 同上全库，测 SQLite LIKE 全量规模延迟 |

## 会话消息语料（15,752 docs / 10.3MB）

| 查询 | FTS5 trigram 中位 | FTS5 p95 | Tantivy ngram(1,3) 中位 | Tantivy p95 |
|---|---|---|---|---|
| 单字符 `a` | 23.76ms（LIKE 兜底） | 25.86ms | **0.02ms** | 0.02ms |
| 单字符 `的` | 28.12ms（LIKE 兜底） | 30.17ms | **0.02ms** | 0.02ms |
| 常用词 `error` | **0.09ms** | 0.14ms | 0.61ms | 0.89ms |
| 中文词 `配置`（2字） | 32.47ms（LIKE 兜底） | 34.27ms | **0.07ms** | 0.15ms |
| 中文短语 `帮我看看` | **0.03ms** | 0.06ms | 0.03ms | 0.04ms |
| AND `nginx proxy` | 0.10ms | 0.20ms | 0.24ms | 0.32ms |
| 冷僻词 `xyzzyplugh` | 0.02ms | 0.04ms | 0.02ms | 0.03ms |

| | FTS5 trigram | Tantivy ngram(1,3) |
|---|---|---|
| 索引耗时 | **2.19s** | 3.34s |
| 索引体积 | 74.5MB（原文 7.2×） | **34.2MB（3.3×）** |

## 文件名语料（100,000 条）

| 查询 | FTS5 trigram 中位 | Tantivy 中位 |
|---|---|---|
| 单字符 `m` | 12.56ms（LIKE 兜底） | **0.04ms** |
| 常用词 `index` | **0.15ms** | 0.58ms |
| 中文 `报告`（2字） | 11.89ms（LIKE 兜底） | **0.00ms** |
| 扩展名 `.json` | **0.15ms** | 0.70ms |

| | FTS5 trigram | Tantivy |
|---|---|---|
| 索引耗时 | **471ms** | 1059ms |
| 索引体积 | 26.7MB | **17.4MB** |

## 决定性数据点：SQLite LIKE 在全量 1,398,404 条文件名上

| 查询 | 中位延迟 |
|---|---|
| `m` | 754.9ms |
| `报告` | 850.8ms |
| `index` | 922.6ms |
| `.json` | 863.7ms |
| `nginx` | 914.9ms |

**超 SPEC <50ms 预算 15-18 倍。** LIKE 全表扫在百万级文件名上不可接受，无论包不包 FTS。

## 结论：混合架构

**Tantivy 落选**（尽管延迟全面占优）：① 名字场景 1.39M 条时其 ngram(1,3) 索引推算 ~240MB，且引入第二个存储引擎（元数据仍在 SQLite，双写一致性、段合并、mmap 生命周期全是复杂度）；② 依赖树编译 ~90s；③ snippet 需自绘（无内置哨兵高亮）。它解决的是"短词慢"一个问题，而这个问题有更便宜的解法。

**采用：**
1. **会话/内容/skills/记忆** → SQLite FTS5 `trigram case_sensitive 0`（与元数据同库同事务，snippet() 内置哨兵高亮）。≥3 字符查询 0.02-0.20ms，远低于预算。<3 字符降级 LIKE 兜底：当前语料 24-34ms 达标；该语料增长 10× 后会超预算，记录为已知边界（单字符内容搜索是低频操作）。
2. **文件名（延迟关键路径）** → **内存子串索引**（`name_index.rs`，Everything 式设计）：名字小写驻留内存（1.39M 条 ≈ 35MB），子串匹配全词长覆盖、无短词死角；命中后按 rowid 回查 SQLite 补 path/mtime。冷启动从 names.bin 快照装载（不扫库），USN 增量同步。预算内且覆盖 1-2 字符查询——这正是 FTS5+LIKE 和纯 Tantivy 方案在全盘规模都守不住的位置。

### 内存文件名索引全量实测（1,392,269 条，rayon 并行，>20 万条自动启用）

| 查询 | 中位延迟 | 说明 |
|---|---|---|
| `m`（单字符，命中 1.13M） | **27.4ms** | 最坏情况仍在 <50ms 预算内 |
| `报告`（中文 2 字） | 3.4ms | |
| `index` | 4.5ms | |
| `.json` | 7.1ms | |
| `nginx` / `spec` / `claude` | 3.0-3.9ms | |
| 装载：SQLite 全量 | 701ms | 索引重建后一次 |
| 装载：names.bin 快照 | **88ms** | 冷启动路径（预算 <500ms ✓） |
| 快照落盘 | 84ms | 后台每 5 分钟 + 索引完成后 |

对照：同语料 SQLite LIKE（Python 内核现行方案）750-920ms，**提速 20-250 倍**。

## SPEC 延迟预算实测（criterion + 实机）

criterion 合成语料：50k 文件名 + 2k 会话消息 + 500 skills/记忆 + 2k 内容文档。
复现：`cargo bench -p anyfind-core`。实机：anyfindd :8790，真实索引（329 会话 / 1434 skills / 27 记忆）。

| 指标 | 预算 | 实测 | 判定 |
|---|---|---|---|
| 文件名击键→结果 | <50ms | 1.39M 全量内存索引：3-7ms，最坏单字符 27.4ms（rayon 并行）；criterion 50k：1.67ms；HTTP 端到端 15ms | ✓ |
| 冷启动到可搜（索引已建） | <500ms | **194ms**（names.bin 快照 88ms + HTTP 就绪） | ✓ |
| 驻留内存 | <150MB | 15.3MB RSS（目录模式，135 文件）；全盘模式估算 +35MB（names） | ✓ |
| 统一搜索 took_ms（实机） | — | 常用词 2-12ms；中文 2 字（LIKE 兜底）20-25ms；组合字段 ≤27ms | ✓ |
| 首次全量索引（会话+记忆，实机） | — | ~35s（329 会话 15.7k 消息含 zstd + 1434 skills + 27 记忆） | 参考值 |
| 首次全量索引（MFT 全盘） | <60s（106 万） | **21.5s（C 盘 172 万文件/32.6 万目录：枚举 6.3s + 写库 15.3s，提权实测）** | ✓ |
| names.bin 快照（172 万条） | — | 97.4MB，落盘 107ms，冷启动装载 155ms（库内全量装载 960ms） | ✓ |
| USN 增量监听 | — | create / rename / delete 三类事件全部捕获（提权实测，200ms 轮询） | ✓ |
| 降级路径 | — | 未提权 enumerate_volume→None，自动降级目录遍历（实测） | ✓ |
| DSL 解析 | — | 简单 0.45µs / 全字段组合 5.3µs | ✓ |

### MFT/USN 提权实测记录（2026-10-21，C 盘，管理员 PowerShell Start-Process -Verb RunAs）

- 提权路径：gsudo 未装；Windows 原生 sudo 被组策略禁用；`Start-Process -Verb RunAs` 可用（同 elevate.py 的 ShellExecuteW runas 机制）。
- 实测中发现并修复两个 windows-rs 结构体坑（Python ctypes 版未踩到）：
  1. `FSCTL_ENUM_USN_DATA`：本机（Win11 24H2）拒绝 32B 的 MFT_ENUM_DATA_V1 输入（ERROR_INVALID_PARAMETER 87），24B 的 V0 正常 → 用 V0。
  2. `FSCTL_READ_USN_JOURNAL`：READ_USN_JOURNAL_DATA_V0 实为 40 字节（UsnJournalID 在 @32，不是 @24），错位报 ERROR_INVALID_USER_BUFFER 1784；且 METHOD_NEITHER 要求输出缓冲 4096 对齐（大 Vec 天然页对齐，小缓冲需显式对齐）。
- 写库优化：关同步 + 内存 journal + 暂缓二级索引 + 清卷裸插 + ASCII 跳过拼音查表，172 万行从 116s → **15.3s**（5.4× → 7.6×）。
- USN 监听上下文复用全量枚举的 FRN 表（`VolumeWatch::from_parts`），不再每卷二次枚举（省 ~10s/卷）。
- daemon 提权整跑（`anyfindd` 本体）：UAC 被用户拒绝 3 次，未做端到端验证；其调用的每个组件（枚举/写库/快照/USN 监听）均以同代码路径在 mftfull 提权实测通过。
- 复现：`cargo run --release -p bench-engine --example mftfull`（提权运行写 target/mft-elevated-result.json）。

criterion 明细（合成语料，并行组 + 连接池）：

| 场景 | 中位 |
|---|---|
| 单字符 `m` | 1.95ms |
| 常用词 `nginx` | 4.61ms |
| 中文 `配置` | 1.54ms |
| 中文短语 `"反向代理"` | 1.69ms |
| AND 两词 | 4.00ms |
| OR | 4.59ms |
| 排除 `-` | 4.42ms |
| 字段过滤组合 | 0.59ms |
| role 过滤 | 1.93ms |
| ext 过滤 | 8.32ms |
| 文件名-内存索引 vs SQL LIKE（50k） | 1.67ms vs 5.81ms |

## 风险与边界

- FTS5 trigram 索引体积 7.2× 原文：会话语料全量约 75MB，可接受；内容索引（20MB/文件上限 + 白名单目录）需控制接入范围。
- 单字符内容搜索（LIKE 兜底）随语料线性增长，150k 消息时约 300ms。如成为痛点，再把内容面切 Tantivy 或加 ngram 辅助表——有本报告数据可对照。
- names.bin 与 index.db 的一致性靠 USN 监听维持；监听失败时降级为下次重建。
