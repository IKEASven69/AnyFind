//! USN journal 增量监听（替代 watchdog，规格硬要求）。
//! 轮询 FSCTL_READ_USN_JOURNAL，把变更记录翻译成 (删除路径, 新增/更新 Entry)
//! 回调给上层写库。FRN 表来自 mft::enumerate_volume 的全量枚举。

use crate::mft::{self, Entry, JournalState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::System::IO::DeviceIoControl;

// USN_REASON 位
const USN_REASON_DATA_OVERWRITE: u32 = 0x00000001;
const USN_REASON_DATA_EXTEND: u32 = 0x00000002;
const USN_REASON_DATA_TRUNCATION: u32 = 0x00000004;
const USN_REASON_NAMED_DATA_OVERWRITE: u32 = 0x00000010;
const USN_REASON_NAMED_DATA_EXTEND: u32 = 0x00000020;
const USN_REASON_NAMED_DATA_TRUNCATION: u32 = 0x00000040;
const USN_REASON_FILE_CREATE: u32 = 0x00000100;
const USN_REASON_FILE_DELETE: u32 = 0x00000200;
const USN_REASON_RENAME_OLD_NAME: u32 = 0x00001000;
const USN_REASON_RENAME_NEW_NAME: u32 = 0x00002000;
const USN_REASON_BASIC_INFO_CHANGE: u32 = 0x00008000;
const USN_REASON_CLOSE: u32 = 0x80000000;

const CHANGE_MASK: u32 = USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_TRUNCATION
    | USN_REASON_NAMED_DATA_OVERWRITE
    | USN_REASON_NAMED_DATA_EXTEND
    | USN_REASON_NAMED_DATA_TRUNCATION
    | USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | USN_REASON_RENAME_OLD_NAME
    | USN_REASON_RENAME_NEW_NAME
    | USN_REASON_BASIC_INFO_CHANGE;

#[derive(Debug, Clone)]
pub struct UsnRecord {
    pub frn: u64,
    pub parent_frn: u64,
    pub name: String,
    pub reason: u32,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub enum Change {
    /// 新建 / 内容变更 / 改名到（重新 stat 入库）
    Upsert(Entry),
    /// 删除 / 改名自（出库）
    Remove(String),
}

/// READ_USN_JOURNAL_DATA_V0 输入（40 字节）：StartUsn i64 @0, ReasonMask u32 @8,
/// ReturnOnlyOnClose u32 @12, Timeout u64 @16, BytesToWaitFor u64 @24, UsnJournalID u64 @32。
/// （UsnJournalID 错放到 @24 会报 ERROR_INVALID_USER_BUFFER 1784 —— 实测）
pub fn read_journal(
    h: windows::Win32::Foundation::HANDLE,
    state: JournalState,
    start_usn: i64,
) -> Option<(Vec<UsnRecord>, i64)> {
    let mut input = [0u8; 40];
    input[0..8].copy_from_slice(&start_usn.to_le_bytes());
    input[8..12].copy_from_slice(&CHANGE_MASK.to_le_bytes());
    // ReturnOnlyOnClose=0, Timeout=0, BytesToWaitFor=0（立即返回当前可用记录）
    input[32..40].copy_from_slice(&state.journal_id.to_le_bytes());

    const BUF_SIZE: usize = 1024 * 1024;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            mft::FSCTL_READ_USN_JOURNAL,
            Some(input.as_ptr() as *const _),
            input.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            buf.len() as u32,
            Some(&mut ret),
            None,
        )
    };
    if ok.is_err() || (ret as usize) < 8 {
        return None;
    }
    let next_usn = i64::from_le_bytes(buf[0..8].try_into().ok()?);

    // 记录区从偏移 8 开始，同 USN_RECORD 布局
    let mut records = Vec::new();
    let nbytes = ret as usize;
    let mut offset = 8usize;
    while offset + 8 <= nbytes {
        let rec_len = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
        if rec_len == 0 || offset + rec_len > nbytes {
            break;
        }
        let rec = &buf[offset..offset + rec_len];
        let major = u16::from_le_bytes([rec[4], rec[5]]);
        if major == 2 || major == 3 {
            let frn = u64::from_le_bytes(rec[8..16].try_into().unwrap()) & 0xFFFFFFFFFFFF;
            let parent = u64::from_le_bytes(rec[16..24].try_into().unwrap()) & 0xFFFFFFFFFFFF;
            let reason = u32::from_le_bytes(rec[40..44].try_into().unwrap());
            let attrs = u32::from_le_bytes(rec[52..56].try_into().unwrap());
            let name_len = u16::from_le_bytes([rec[56], rec[57]]) as usize;
            let name_off = u16::from_le_bytes([rec[58], rec[59]]) as usize;
            if name_len > 0 && name_off + name_len <= rec.len() {
                let units: Vec<u16> = (0..name_len / 2)
                    .map(|i| u16::from_le_bytes([rec[name_off + i * 2], rec[name_off + i * 2 + 1]]))
                    .collect();
                if let Ok(name) = String::from_utf16(&units) {
                    records.push(UsnRecord {
                        frn,
                        parent_frn: parent,
                        name,
                        reason,
                        is_dir: attrs & 0x10 != 0,
                    });
                }
            }
        }
        offset += rec_len;
    }
    Some((records, next_usn))
}

/// 诊断版 journal 读取：返回 FSCTL 调用结果与错误码（排查 USN 监听参数问题）。
pub fn diagnose_read(drive: &str) -> String {
    let Some(h) = mft::open_volume(drive) else {
        return format!("open_volume failed: {:?}", std::io::Error::last_os_error());
    };
    let Some(state) = mft::query_journal(h) else {
        unsafe { let _ = windows::Win32::Foundation::CloseHandle(h); }
        return format!("query_journal failed: {:?}", std::io::Error::last_os_error());
    };

    // 对齐分配辅助：4096 对齐的输出缓冲（METHOD_NEITHER 对对齐敏感）
    fn aligned_buf(size: usize, align: usize) -> (*mut u8, Vec<u8>) {
        let raw = vec![0u8; size + align];
        let off = raw.as_ptr().align_offset(align);
        (unsafe { raw.as_ptr().add(off) as *mut u8 }, raw)
    }

    let mut out = Vec::new();
    for (desc, in_len, ver, use_aligned, out_mb) in [
        ("V0-40B/unaligned/1MB", 40usize, (0u32, 0u32), false, 1usize),
        ("V0-40B/aligned4096/1MB", 40, (0, 0), true, 1),
        ("V1-48B/2-3/aligned/1MB", 48, (2, 3), true, 1),
    ] {
        let mut input = [0u8; 48];
        input[0..8].copy_from_slice(&state.next_usn.to_le_bytes());
        input[8..12].copy_from_slice(&CHANGE_MASK.to_le_bytes());
        input[32..40].copy_from_slice(&state.journal_id.to_le_bytes());
        input[40..44].copy_from_slice(&ver.0.to_le_bytes());
        input[44..48].copy_from_slice(&ver.1.to_le_bytes());
        let (ptr, _keep) = if use_aligned {
            aligned_buf(out_mb * 1024 * 1024, 4096)
        } else {
            let v = vec![0u8; out_mb * 1024 * 1024];
            let p = v.as_ptr() as *mut u8;
            (p, v)
        };
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                mft::FSCTL_READ_USN_JOURNAL,
                Some(input.as_ptr() as *const _),
                in_len as u32,
                Some(ptr as *mut _),
                (out_mb * 1024 * 1024) as u32,
                Some(&mut ret),
                None,
            )
        };
        match ok {
            Ok(()) => out.push(format!("{desc}: OK ret={ret}")),
            Err(_) => out.push(format!(
                "{desc}: ERR {}",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
            )),
        }
    }
    unsafe { let _ = windows::Win32::Foundation::CloseHandle(h); }
    format!("journal=(id={}, next={})\n  {}", state.journal_id, state.next_usn, out.join("\n  "))
}

/// 一个卷的监听上下文：FRN 表 + 路径缓存 + journal 位置。
pub struct VolumeWatch {
    pub drive: String,
    pub frn_map: HashMap<u64, (u64, String, bool)>,
    pub path_cache: HashMap<u64, Option<String>>,
    pub journal: JournalState,
    pub cursor: i64,
}

impl VolumeWatch {
    pub fn new(drive: &str) -> Option<Self> {
        let drive = drive.trim_end_matches('\\').to_string();
        let (_entries, frn_map, journal) = mft::enumerate_volume(&drive)?;
        Some(VolumeWatch {
            drive,
            frn_map,
            path_cache: HashMap::new(),
            journal,
            cursor: journal.next_usn,
        })
    }

    /// 复用已有的全量枚举结果（避免每卷二次枚举，daemon 启动路径用）。
    pub fn from_parts(
        drive: &str,
        frn_map: HashMap<u64, (u64, String, bool)>,
        journal: JournalState,
    ) -> Self {
        VolumeWatch {
            drive: drive.trim_end_matches('\\').to_string(),
            frn_map,
            path_cache: HashMap::new(),
            journal,
            cursor: journal.next_usn,
        }
    }

    fn full_path(&mut self, frn: u64) -> Option<String> {
        mft::build_path(frn, &self.frn_map, &mut self.path_cache, &self.drive)
    }

    /// 处理一批记录 → 变更集。同时维护 FRN 表（创建/改名/删除）。
    pub fn apply(&mut self, records: &[UsnRecord]) -> Vec<Change> {
        let mut changes = Vec::new();
        for r in records {
            let reason = r.reason;
            if reason & USN_REASON_RENAME_OLD_NAME != 0 {
                if let Some(old) = self.full_path(r.frn) {
                    changes.push(Change::Remove(old));
                }
                // 名字即将变，先让缓存失效
                self.path_cache.remove(&r.frn);
            }
            if reason & USN_REASON_FILE_DELETE != 0 {
                if let Some(old) = self.full_path(r.frn) {
                    changes.push(Change::Remove(old));
                }
                self.frn_map.remove(&r.frn);
                self.path_cache.remove(&r.frn);
                continue;
            }
            // 创建 / 内容变更 / 改名新名 / 元信息变更 → 更新 FRN 表 + upsert
            if reason
                & (USN_REASON_FILE_CREATE
                    | USN_REASON_RENAME_NEW_NAME
                    | USN_REASON_DATA_OVERWRITE
                    | USN_REASON_DATA_EXTEND
                    | USN_REASON_DATA_TRUNCATION
                    | USN_REASON_BASIC_INFO_CHANGE
                    | USN_REASON_CLOSE)
                != 0
            {
                if reason & (USN_REASON_FILE_CREATE | USN_REASON_RENAME_NEW_NAME) != 0 {
                    self.frn_map.insert(r.frn, (r.parent_frn, r.name.clone(), r.is_dir));
                    self.path_cache.remove(&r.frn);
                }
                if let Some(path) = self.full_path(r.frn) {
                    // 从磁盘补 size（MFT 记录无精确 size）
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    changes.push(Change::Upsert(Entry {
                        path,
                        name: r.name.clone(),
                        size,
                        is_dir: r.is_dir,
                    }));
                }
            }
        }
        changes
    }
}

/// 监听循环：每 interval_ms 轮询一次 journal，变更回调 on_change。
/// stop 置 true 时退出。任何一步失败（journal 回卷/句柄失效）返回 false，
/// 调用方应降级重建或放弃监听。
pub fn watch_volume<F: FnMut(Vec<Change>)>(
    watch: &mut VolumeWatch,
    stop: Arc<AtomicBool>,
    interval_ms: u64,
    mut on_change: F,
) -> bool {
    let Some(h) = mft::open_volume(&watch.drive) else {
        return false;
    };
    loop {
        if stop.load(Ordering::Relaxed) {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
            return true;
        }
        match read_journal(h, watch.journal, watch.cursor) {
            Some((records, next)) => {
                watch.cursor = next;
                if !records.is_empty() {
                    let changes = watch.apply(&records);
                    if !changes.is_empty() {
                        on_change(changes);
                    }
                }
            }
            None => {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(h);
                }
                return false;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
}
