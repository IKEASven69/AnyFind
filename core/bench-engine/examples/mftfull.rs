//! MFT/USN 全盘路径实测（提权跑）。
//! 未提权运行：验证降级路径（enumerate_volume 必须返回 None），exit 2。
//! 提权运行：C 盘全量枚举耗时 → build_mft_index 写库 → names.bin 快照/装载 →
//! USN 监听捕获 create/rename/delete → JSON 报告写 target/mft-elevated-result.json。

use anyfind_core::{db, mft, name_index::NameIndex, scanner, usn};
use std::sync::mpsc;
use std::time::Instant;

fn log_line(msg: &str) {
    use std::io::Write;
    let p = std::path::Path::new(r"D:\coding\anyfind\core\target\mft-elevated.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{}", msg);
    }
    println!("{msg}");
}

fn main() {
    let _ = std::fs::remove_file(r"D:\coding\anyfind\core\target\mft-elevated.log");
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("PANIC: {info}"));
    }));
    log_line("mftfull start");
    let elevated = mft::is_elevated();
    log_line(&format!("elevated = {elevated}"));
    let volumes = mft::list_ntfs_volumes();
    log_line(&format!("ntfs volumes = {volumes:?}"));

    if !elevated {
        // 降级路径验证：未提权时枚举必须返回 None（调用方降级目录遍历）
        let r = mft::enumerate_volume("C:");
        log_line(&format!("enumerate_volume(C:) unelevated = {}", if r.is_none() { "None (degrade OK)" } else { "SOME (BUG!)" }));
        log_line(&format!("diagnose: {}", mft::diagnose_enumerate("C:")));
        std::process::exit(if r.is_none() { 2 } else { 1 });
    }

    log_line(&format!("diagnose: {}", mft::diagnose_enumerate("C:")));

    let mut report = serde_json::json!({"volumes": volumes});

    // ---- 1. C 盘全量枚举 ----
    let t0 = Instant::now();
    let (entries, _frn_map, journal) = mft::enumerate_volume("C:").expect("C: enumerate failed as admin");
    let enum_ms = t0.elapsed().as_millis();
    let files = entries.iter().filter(|e| !e.is_dir).count();
    let dirs = entries.iter().filter(|e| e.is_dir).count();
    println!("C: enumerate: {enum_ms}ms, {files} files / {dirs} dirs, journal next_usn={}", journal.next_usn);
    report["enumerate_ms"] = (enum_ms as u64).into();
    report["files"] = files.into();
    report["dirs"] = dirs.into();

    // ---- 2. build_mft_index 写库（daemon 实际路径） ----
    let tmp = std::env::temp_dir().join("anyfind-mft-test.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = db::open(&tmp).unwrap();
    db::ensure_tables(&conn).unwrap();
    let t0 = Instant::now();
    let (stats, degraded) = scanner::build_mft_index(&conn, Some(vec!["C:".into()]));
    let index_ms = t0.elapsed().as_millis();
    println!("build_mft_index: {index_ms}ms, stats={stats:?}, degraded={degraded:?}");
    report["build_mft_index_ms"] = (index_ms as u64).into();
    report["degraded"] = format!("{degraded:?}").into();

    // ---- 3. names.bin 快照与冷启动装载 ----
    let t0 = Instant::now();
    let idx = NameIndex::load_from_db(&conn).unwrap();
    let load_db_ms = t0.elapsed().as_millis();
    let blob = std::env::temp_dir().join("anyfind-mft-names.bin");
    let t0 = Instant::now();
    idx.save_blob(&blob).unwrap();
    let save_ms = t0.elapsed().as_millis();
    let blob_bytes = std::fs::metadata(&blob).map(|m| m.len()).unwrap_or(0);
    let t0 = Instant::now();
    let idx2 = NameIndex::load_blob(&blob).unwrap();
    let load_blob_ms = t0.elapsed().as_millis();
    println!("names: load_db={load_db_ms}ms save={save_ms}ms ({blob_bytes}B) load_blob={load_blob_ms}ms entries={}", idx2.entries.len());
    report["names_load_db_ms"] = (load_db_ms as u64).into();
    report["names_save_ms"] = (save_ms as u64).into();
    report["names_blob_bytes"] = blob_bytes.into();
    report["names_load_blob_ms"] = (load_blob_ms as u64).into();

    // ---- 4. USN 监听：create / rename / delete ----
    log_line(&format!("usn diagnose: {}", usn::diagnose_read("C:")));
    println!("building USN watch context (re-enumerate)…");
    let t0 = Instant::now();
    let Some(mut watch) = usn::VolumeWatch::new("C:") else {
        eprintln!("VolumeWatch::new failed");
        std::process::exit(1);
    };
    println!("watch context ready in {}ms", t0.elapsed().as_millis());
    report["watch_ctx_ms"] = (t0.elapsed().as_millis() as u64).into();

    let (tx, rx) = mpsc::channel::<Vec<usn::Change>>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || {
        usn::watch_volume(&mut watch, stop2, 200, move |changes| {
            let _ = tx.send(changes);
        });
    });

    // 探针文件（C: 上的 %TEMP%）
    let probe_dir = std::env::temp_dir().join("anyfind-usn-probe");
    std::fs::create_dir_all(&probe_dir).unwrap();
    let a = probe_dir.join("probe-a.txt");
    let b = probe_dir.join("probe-b.txt");
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::fs::write(&a, b"anyfind usn probe").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(700));
    std::fs::rename(&a, &b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(700));
    std::fs::remove_file(&b).unwrap();
    std::fs::remove_dir(&probe_dir).unwrap();

    // 收集 8 秒事件
    let mut upserts = Vec::new();
    let mut removes = Vec::new();
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(batch) => {
                for ch in batch {
                    match ch {
                        usn::Change::Upsert(e) if e.path.contains("anyfind-usn-probe") => {
                            upserts.push(e.path.clone())
                        }
                        usn::Change::Remove(p) if p.contains("anyfind-usn-probe") => {
                            removes.push(p.clone())
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => {
                if !upserts.is_empty() && !removes.is_empty() {
                    break;
                }
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = handle.join();
    println!("USN upserts={upserts:?}");
    println!("USN removes={removes:?}");
    let saw_create = upserts.iter().any(|p| p.ends_with("probe-a.txt"));
    let saw_rename = upserts.iter().any(|p| p.ends_with("probe-b.txt"));
    let saw_delete = removes.iter().any(|p| p.ends_with("probe-b.txt"));
    report["usn_create"] = saw_create.into();
    report["usn_rename"] = saw_rename.into();
    report["usn_delete"] = saw_delete.into();
    println!(
        "USN verdict: create={} rename={} delete={}",
        saw_create, saw_rename, saw_delete
    );

    let out = std::path::Path::new(r"D:\coding\anyfind\core\target\mft-elevated-result.json");
    std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("report written: {}", out.display());
}
