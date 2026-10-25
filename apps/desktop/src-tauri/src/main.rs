// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    kill_zombie_webviews();
    anyfind_desktop_lib::run()
}

/// 启动前清理本应用 profile 的僵尸 WebView2 进程。
/// 背景：异常退出可能留下持有 %LOCALAPPDATA%/com.anyfind.desktop/EBWebView
/// 锁的孤儿 msedgewebview2，新实例创建 webview 时报 0x800700AA（资源使用中）崩溃。
/// 仅在无任何 AnyFind 实例运行时清理（有实例则这些 webview 属于它，不能动）。
#[cfg(windows)]
fn kill_zombie_webviews() {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "anyfind-desktop-zombie-guard\0".encode_utf16().collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if h.is_null() {
        return;
    }
    // 互斥体已被持有 = 另一个实例正在运行 → 它的 webview 不是僵尸
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running {
        unsafe { CloseHandle(h) };
        return;
    }

    let mut sys = sysinfo::System::new();
    sys.refresh_processes();
    let mut killed = 0u32;
    for (_pid, p) in sys.processes() {
        let is_wv = p.name().eq_ignore_ascii_case("msedgewebview2.exe");
        if !is_wv {
            continue;
        }
        let cmdline = p.cmd().join(" ");
        if cmdline.contains("com.anyfind.desktop") {
            if p.kill() {
                killed += 1;
            }
        }
    }
    if killed > 0 {
        eprintln!("[anyfind] 清理僵尸 WebView2 进程 {} 个", killed);
    }
    // 注意：不 CloseHandle —— 互斥体持有到进程退出，
    // 防止后启动的第二实例把本实例的活跃 webview 误当僵尸清理。
    std::mem::forget(h);
}
