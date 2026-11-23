//! Windows MFT 全盘文件枚举（FSCTL_ENUM_USN_DATA，Everything 同款技术）。
//! 移植自前身 Python 参考实现 mft_index.py。需要管理员权限；非 NTFS / 权限不足返回 None，
//! 调用方降级目录遍历（scanner::build_dir_index）。

use std::collections::HashMap;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetLogicalDrives, GetVolumeInformationW, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

pub const FSCTL_ENUM_USN_DATA: u32 = 0x000900B3;
pub const FSCTL_QUERY_USN_JOURNAL: u32 = 0x000900F4;
pub const FSCTL_READ_USN_JOURNAL: u32 = 0x000900BB;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct JournalState {
    pub journal_id: u64,
    pub next_usn: i64,
    pub max_usn: i64,
}

/// 当前进程是否已提权（管理员）。
pub fn is_elevated() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && elev.TokenIsElevated != 0
    }
}

pub fn open_volume(drive: &str) -> Option<HANDLE> {
    let vol = format!(r"\\.\{}", drive.trim_end_matches('\\'));
    let wide: Vec<u16> = vol.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
    }
}

fn fs_type(drive: &str) -> String {
    let root = format!("{}\\", drive.trim_end_matches('\\'));
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut fs_buf = [0u16; 256];
    unsafe {
        let ok = GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs_buf),
        );
        if ok.is_err() {
            return String::new();
        }
    }
    let end = fs_buf.iter().position(|&x| x == 0).unwrap_or(fs_buf.len());
    String::from_utf16_lossy(&fs_buf[..end])
}

/// 返回所有 NTFS 盘盘符，如 ["C:", "D:"]。
pub fn list_ntfs_volumes() -> Vec<String> {
    let mut out = Vec::new();
    let bits = unsafe { GetLogicalDrives() };
    for i in 0..26 {
        if bits & (1 << i) != 0 {
            let letter = format!("{}:", (b'A' + i as u8) as char);
            if fs_type(&letter).eq_ignore_ascii_case("NTFS") {
                out.push(letter);
            }
        }
    }
    out
}

#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn rd_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}

/// USN_JOURNAL_DATA_V0（56 字节）
pub fn query_journal(h: HANDLE) -> Option<JournalState> {
    let mut out = [0u8; 80];
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(out.as_mut_ptr() as *mut _),
            out.len() as u32,
            Some(&mut ret),
            None,
        )
    };
    if ok.is_err() || (ret as usize) < 56 {
        return None;
    }
    Some(JournalState {
        journal_id: rd_u64(&out, 0),
        next_usn: rd_u64(&out, 16) as i64, // NextUsn @16
        max_usn: rd_u64(&out, 32) as i64,  // MaxUsn @32（V0 布局：id,First,Next,Lowest,Max）
    })
}

/// 解析 USN_RECORD 序列（缓冲区前 8 字节是 NextUsn/下一 FRN）。
/// 返回 (frn, parent_frn, name, is_dir)。
fn parse_usn_records(buf: &[u8], nbytes: usize) -> Vec<(u64, u64, String, bool)> {
    let mut out = Vec::new();
    let mut offset = 8usize;
    while offset + 8 <= nbytes {
        let rec_len = rd_u32(buf, offset) as usize;
        if rec_len == 0 || offset + rec_len > nbytes {
            break;
        }
        let major = rd_u16(buf, offset + 4);
        if major == 2 || major == 3 {
            let rec = &buf[offset..offset + rec_len];
            let frn = rd_u64(rec, 8) & 0xFFFFFFFFFFFF;
            let parent = rd_u64(rec, 16) & 0xFFFFFFFFFFFF;
            let file_attr = rd_u32(rec, 52);
            let name_len = rd_u16(rec, 56) as usize;
            let name_off = rd_u16(rec, 58) as usize;
            if name_len > 0 && name_off + name_len <= rec.len() {
                let units: Vec<u16> = (0..name_len / 2)
                    .map(|i| rd_u16(rec, name_off + i * 2))
                    .collect();
                if let Ok(name) = String::from_utf16(&units) {
                    out.push((frn, parent, name, file_attr & FILE_ATTRIBUTE_DIRECTORY != 0));
                }
            }
        }
        offset += rec_len;
    }
    out
}

/// 诊断版枚举：逐阶段 + 输入结构变体矩阵（排查 FSCTL_ENUM_USN_DATA 参数问题）。
pub fn diagnose_enumerate(drive: &str) -> String {
    let drive = drive.trim_end_matches('\\');
    let fs = fs_type(drive);
    if !fs.eq_ignore_ascii_case("NTFS") {
        return format!("fs_type={fs:?} not NTFS");
    }
    let Some(h) = open_volume(drive) else {
        return format!("open_volume failed (err={:?})", std::io::Error::last_os_error());
    };
    let Some(state) = query_journal(h) else {
        let e = std::io::Error::last_os_error();
        unsafe { let _ = CloseHandle(h); }
        return format!("query_journal failed (err={e:?})");
    };

    let mut results = Vec::new();
    // 变体: (描述, 输入字节数, LowUsn, HighUsn, MinVer, MaxVer, 输出缓冲)
    let variants: Vec<(&str, usize, i64, i64, u32, u32, usize)> = vec![
        ("V1-32B/high=max/1MB", 32, 0, state.max_usn, 2, 3, 1024 * 1024),
        ("V1-32B/high=next/1MB", 32, 0, state.next_usn, 2, 3, 1024 * 1024),
        ("V1-32B/2,2/1MB", 32, 0, state.max_usn, 2, 2, 1024 * 1024),
        ("V1-32B/3,3/1MB", 32, 0, state.max_usn, 3, 3, 1024 * 1024),
        ("V0-24B/1MB", 24, 0, state.max_usn, 0, 0, 1024 * 1024),
        ("V1-32B/64KB", 32, 0, state.max_usn, 2, 3, 64 * 1024),
    ];
    for (desc, in_len, low, high, minv, maxv, out_size) in variants {
        let mut med = [0u8; 32];
        med[8..16].copy_from_slice(&low.to_le_bytes());
        med[16..24].copy_from_slice(&high.to_le_bytes());
        med[24..28].copy_from_slice(&minv.to_le_bytes());
        med[28..32].copy_from_slice(&maxv.to_le_bytes());
        let mut buf = vec![0u8; out_size];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_ENUM_USN_DATA,
                Some(med.as_ptr() as *const _),
                in_len as u32,
                Some(buf.as_mut_ptr() as *mut _),
                buf.len() as u32,
                Some(&mut ret),
                None,
            )
        };
        let msg = match ok {
            Ok(()) => format!("OK ret={ret} recs={}", parse_usn_records(&buf, ret as usize).len()),
            Err(_) => {
                let e = std::io::Error::last_os_error();
                format!("ERR {} (ret={ret})", e.raw_os_error().unwrap_or(-1))
            }
        };
        results.push(format!("{desc}: {msg}"));
    }
    unsafe { let _ = CloseHandle(h); }
    format!(
        "journal=(id={}, next={}, max={})\n  {}",
        state.journal_id,
        state.next_usn,
        state.max_usn,
        results.join("\n  ")
    )
}

/// 枚举一个 NTFS 卷的全部文件/目录（FRN 表 + 路径重建）。
/// 返回 (entries, frn_map, journal_state)；非 NTFS/无权限/journal 缺失返回 None。
/// frn_map 供 USN 增量监听复用（frn → (parent_frn, name, is_dir)）。
pub fn enumerate_volume(
    drive: &str,
) -> Option<(Vec<Entry>, HashMap<u64, (u64, String, bool)>, JournalState)> {
    let drive = drive.trim_end_matches('\\');
    if !fs_type(drive).eq_ignore_ascii_case("NTFS") {
        return None;
    }
    let h = open_volume(drive)?;
    let state = query_journal(h)?;

    // MFT_ENUM_DATA_V0（24B：StartFRN/LowUsn/HighUsn）。实测本机（Win11 24H2）
    // 拒绝 32B 的 V1 输入（ERROR_INVALID_PARAMETER 87），V0 正常 —— 见 bench-001。
    let mut med = [0u8; 24];
    med[16..24].copy_from_slice(&state.max_usn.to_le_bytes()); // HighUsn

    const BUF_SIZE: usize = 1024 * 1024;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut frn_map: HashMap<u64, (u64, String, bool)> = HashMap::with_capacity(1 << 20);
    let mut empty_batches = 0;

    loop {
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                FSCTL_ENUM_USN_DATA,
                Some(med.as_ptr() as *const _),
                med.len() as u32,
                Some(buf.as_mut_ptr() as *mut _),
                buf.len() as u32,
                Some(&mut ret),
                None,
            )
        };
        if ok.is_err() {
            break; // ERROR_HANDLE_EOF 等：枚举完成
        }
        if (ret as usize) <= 8 {
            empty_batches += 1;
            if empty_batches >= 2 {
                break;
            }
            continue;
        }
        empty_batches = 0;
        for (frn, parent, name, is_dir) in parse_usn_records(&buf, ret as usize) {
            frn_map.insert(frn, (parent, name, is_dir));
        }
        let next_start = rd_u64(&buf, 0);
        let cur_start = rd_u64(&med, 0);
        if next_start == cur_start {
            break; // 未推进，枚举完成
        }
        med[0..8].copy_from_slice(&next_start.to_le_bytes());
    }
    unsafe {
        let _ = CloseHandle(h);
    }
    if frn_map.is_empty() {
        return None; // 正常 NTFS 卷不可能 0 条：视为失败让调用方降级
    }

    let mut path_cache: HashMap<u64, Option<String>> = HashMap::with_capacity(frn_map.len());
    let mut entries = Vec::with_capacity(frn_map.len());
    let frns: Vec<u64> = frn_map.keys().copied().collect();
    for frn in frns {
        if let Some(path) = build_path(frn, &frn_map, &mut path_cache, drive) {
            let (_, name, is_dir) = &frn_map[&frn];
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            entries.push(Entry { path, name: name.clone(), size: 0, is_dir: *is_dir });
        }
    }
    Some((entries, frn_map, state))
}

/// 自底向上用 frn_map 还原完整路径（父 FRN=5 是卷根）。
pub fn build_path(
    frn: u64,
    map: &HashMap<u64, (u64, String, bool)>,
    cache: &mut HashMap<u64, Option<String>>,
    drive: &str,
) -> Option<String> {
    if let Some(c) = cache.get(&frn) {
        return c.clone();
    }
    cache.insert(frn, None); // 防循环
    let result = match map.get(&frn) {
        None => None,
        Some((parent, name, _)) => {
            if *parent == 5 || *parent == 0 || *parent == frn {
                Some(format!("{drive}\\{name}"))
            } else {
                match build_path(*parent, map, cache, drive) {
                    Some(p) => Some(format!("{p}\\{name}")),
                    None => Some(format!("{drive}\\{name}")),
                }
            }
        }
    };
    cache.insert(frn, result.clone());
    result
}
