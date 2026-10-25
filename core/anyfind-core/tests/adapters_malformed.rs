//! 畸形样本容错测试：坏行跳过不崩（SPEC：proptest 覆盖畸形行容错）。
//! HOME 重定向到临时目录，避免碰真实数据。

use anyfind_core::adapters;
use std::path::Path;

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn write_zstd(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let data = zstd::stream::encode_all(s.as_bytes(), 3).unwrap();
    std::fs::write(p, data).unwrap();
}

#[test]
fn malformed_lines_never_panic() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("USERPROFILE", tmp.path());
    std::env::set_var("HOME", tmp.path());

    // ---- claude：垃圾行 / 截断 JSON / 缺 message / 空 content 块 / 坏 UTF-8 ----
    let claude_f = tmp.path().join(".claude/projects/p/s1.jsonl");
    {
        std::fs::create_dir_all(claude_f.parent().unwrap()).unwrap();
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"not json at all\n");
        bytes.extend_from_slice("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"正常用户消息\"}]},\"timestamp\":\"2026-10-01T00:00:00\",\"cwd\":\"D:/x\"}\n".as_bytes());
        bytes.extend_from_slice(b"{\"type\":\"user\",\"message\":  \n");
        bytes.extend_from_slice("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"纯字符串内容\"},\"timestamp\":\"2026-10-01T00:00:01\"}\n".as_bytes());
        bytes.extend_from_slice(&[0xE4, 0xB8, 0xAD, b' ', b'b', b'r', b'o', b'k', b'e', b'n', b' ', 0xFF, 0xFE, b'\n']);
        bytes.extend_from_slice(b"{\"type\":\"progress\",\"data\":\"skip me\"}\n");
        bytes.extend_from_slice(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"content\":\"\xE5\x9B\x9E\xE6\x89\xAC\"}]}}\n");
        std::fs::write(&claude_f, bytes).unwrap();
    }
    let p = adapters::claude::parse(&claude_f);
    assert_eq!(p.messages.len(), 2, "tool_result 块应跳过，坏行应跳过: {:?}", p.messages);
    assert_eq!(p.cwd, "D:/x");
    assert_eq!(p.title.as_deref(), Some("正常用户消息"));

    // ---- codex：event_msg 跳过 / system 角色跳过 / 截断行 ----
    let codex_f = tmp.path().join(".codex/sessions/2026/10/rollout-9f8e7d6c-1a2b-3c4d-8e9f-0a1b2c3d4e5f.jsonl");
    write(
        &codex_f,
        "{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"D:/proj\"}}\n\
         {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"codex 用户问题\"}]},\"timestamp\":\"t1\"}\n\
         {\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\"}}\n\
         {\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\"}}\n\
         {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"system\",\"content\":[{\"text\":\"noise\"}]}}\n\
         {\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"text\":\"codex 回答\"}]},\"timestamp\":\"t2\"}\n\
         garbage line {{{\n",
    );
    let p = adapters::codex::parse(&codex_f);
    assert_eq!(p.messages.len(), 2);
    assert_eq!(p.cwd, "D:/proj");

    // ---- dsh：zstd 压缩 + chunks 跳过 + 坏行 + session/title ----
    let dsh_dir = tmp.path().join(".dsh/sessions/-D-x/session-aaa111/session.v2.jsonl.zstd");
    write_zstd(
        &dsh_dir,
        "{\"type\":\"session\",\"cwd\":\"D:/x\",\"createdAt\":1760000000000,\"id\":\"aaa111\"}\n\
         {\"type\":\"user/message\",\"time\":1760000001000,\"data\":{\"content\":[{\"type\":\"text\",\"text\":\"dsh 问题\"}]}}\n\
         {\"type\":\"assistant/message-chunks\",\"data\":{}}\n\
         {\"type\":\"session/title\",\"data\":{\"title\":\"dsh 标题\"}}\n\
         {\"type\":\"assistant/message\",\"time\":1760000002000,\"data\":{\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"dsh 回答\"}]}}}\n\
         这不是 json\n\
         {\"type\":\"user/message\",\"data\":\"content 不是对象\"}\n",
    );
    // 再写一个低版本，验证 discover 取最高版本
    write(&tmp.path().join(".dsh/sessions/-D-x/session-aaa111/session.jsonl"), "{\"type\":\"session\"}\n");
    let refs = adapters::dsh::discover();
    assert_eq!(refs.len(), 1, "同目录多版本只取最高版");
    assert!(refs[0].path.ends_with("session.v2.jsonl.zstd"));
    let p = adapters::dsh::parse(&refs[0].path);
    assert_eq!(p.messages.len(), 2);
    assert_eq!(p.cwd, "D:/x");
    assert_eq!(p.title.as_deref(), Some("dsh 标题"));

    // ---- opencode：三层 + 缺 part / 坏 json 文件 ----
    let oc = tmp.path().join(".local/share/opencode/storage");
    write(&oc.join("session/prj1/ses_1.json"), "{\"id\":\"ses_1\",\"directory\":\"D:/oc\",\"title\":\"oc 会话\"}");
    write(&oc.join("session/prj1/ses_bad.json"), "{broken");
    write(&oc.join("message/ses_1/msg_1.json"), "{\"id\":\"msg_1\",\"role\":\"user\",\"time\":{\"created\":1760000001000}}");
    write(&oc.join("message/ses_1/msg_2.json"), "{\"id\":\"msg_2\",\"role\":\"assistant\",\"time\":{\"created\":1760000002000}}");
    write(&oc.join("message/ses_1/msg_garbage.json"), "not json");
    write(&oc.join("part/msg_1/prt_1.json"), "{\"type\":\"text\",\"text\":\"oc 用户消息\"}");
    write(&oc.join("part/msg_2/prt_1.json"), "{\"type\":\"tool\",\"text\":\"工具块跳过\"}");
    write(&oc.join("part/msg_2/prt_2.json"), "{\"type\":\"text\",\"text\":\"oc 回答\"}");
    let refs = adapters::opencode::discover();
    assert_eq!(refs.len(), 2, "坏 session json 也应进 discover（容错）");
    let good = refs.iter().find(|r| r.session_id == "ses_1").unwrap();
    let p = adapters::opencode::parse(&good.path);
    assert_eq!(p.messages.len(), 2, "tool part 跳过、坏 message json 跳过");
    assert_eq!(p.title.as_deref(), Some("oc 会话"));
    let bad = refs.iter().find(|r| r.session_id == "ses_bad").unwrap();
    let p2 = adapters::opencode::parse(&bad.path);
    assert!(p2.messages.is_empty());

    // ---- pi：角色过滤 + 坏行 ----
    let pi_f = tmp.path().join(".pi/agent/sessions/-D-y/1760000000000_9f8e7d6c-1a2b-3c4d-8e9f-0a1b2c3d4e5f.jsonl");
    write(
        &pi_f,
        "{\"type\":\"session\",\"cwd\":\"D:/y\"}\n\
         {\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"pi 问题\"}]},\"timestamp\":\"t1\"}\n\
         {\"type\":\"message\",\"message\":{\"role\":\"tool\",\"content\":[{\"type\":\"text\",\"text\":\"tool 跳过\"}]}}\n\
         broken\n\
         {\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"pi 回答\"}]},\"timestamp\":\"t2\"}\n",
    );
    let refs = adapters::pi::discover();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].session_id, "9f8e7d6c-1a2b-3c4d-8e9f-0a1b2c3d4e5f");
    let p = adapters::pi::parse(&pi_f);
    assert_eq!(p.messages.len(), 2);

    // ---- 增量索引整链路：build_session_index 不崩，可重复跑 ----
    let dbp = tmp.path().join("idx.db");
    let conn = anyfind_core::db::open(&dbp).unwrap();
    let pool = anyfind_core::db::Pool::new(&dbp, 4);
    anyfind_core::db::ensure_tables(&conn).unwrap();
    let stats1 = anyfind_core::db::build_session_index(&conn);
    assert!(stats1["claude-code"].updated >= 1);
    let stats2 = anyfind_core::db::build_session_index(&conn);
    assert_eq!(stats2["claude-code"].updated, 0, "二次应为指纹跳过");
    assert!(stats2["claude-code"].skipped >= 1);
    // 搜索能找到
    let kinds = vec!["session".to_string()];
    let res = anyfind_core::search::unified_search(&pool, None, "正常用户消息", &kinds, 20, 0);
    let sessions = res.groups.get("session").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert!(!sessions.is_empty(), "会话应可被搜到");
    // /api/session 详情
    let detail = anyfind_core::resume::get_session_messages(&conn, "s1", 100);
    assert!(detail.is_some());

    // 重建期间并发读取：旧代或新代都完整，不允许出现 session_id 空窗。
    let mut large = String::new();
    for i in 0..5000 {
        large.push_str(&format!(
            "{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"消息 {i}\"}},\"cwd\":\"D:/x\"}}\n"
        ));
    }
    std::fs::write(&claude_f, large).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let reader = {
        let dbp = dbp.clone();
        let stop = stop.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || -> Result<usize, String> {
            let conn = anyfind_core::db::open(&dbp).unwrap();
            barrier.wait();
            let mut reads = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let detail = anyfind_core::resume::get_session_messages(&conn, "s1", 10);
                match detail {
                    Some(d) if !d.messages.is_empty() => {}
                    _ => return Err("重建中读到了不存在的会话或空消息".into()),
                }
                reads += 1;
            }
            Ok(reads)
        })
    };
    barrier.wait();
    let writer = anyfind_core::db::open(&dbp).unwrap();
    anyfind_core::db::build_session_index(&writer);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let reads = reader.join().unwrap().unwrap();
    assert!(reads > 0);
}

proptest::proptest! {
    #[test]
    fn dsl_never_panics(s in ".*{0,200}") {
        let q = anyfind_core::dsl::parse(&s);
        let _ = anyfind_core::dsl::fts5_query(&q);
        let _ = anyfind_core::dsl::positive_terms(&q);
        if let Some(e) = &q.expr {
            let _ = anyfind_core::dsl::text_matches(e, "haystack 文本 nginx");
        }
    }
}
