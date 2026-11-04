// AnyFind desktop shell — Tauri v2.
// 壳职责：窗口/全局热键/托盘/单实例/开机自启/配置落盘，以及到 anyfindd 内核的
// HTTP 代理（绕过 webview CORS，Python/Rust 内核同契约无缝切换）。

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const DEFAULT_HOTKEY: &str = "ctrl+shift+f";
const DEFAULT_KERNEL: &str = "http://127.0.0.1:8790";
static KERNEL_MIGRATING: AtomicBool = AtomicBool::new(false);

struct KernelMigrationGuard;
impl KernelMigrationGuard {
    fn new() -> Self {
        KERNEL_MIGRATING.store(true, Ordering::SeqCst);
        Self
    }
}
impl Drop for KernelMigrationGuard {
    fn drop(&mut self) {
        KERNEL_MIGRATING.store(false, Ordering::SeqCst);
    }
}

struct AppState {
    hotkey: Mutex<String>,
}

/* ============ 配置（%APPDATA%/anyfind/config.json） ============ */

fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("anyfind").join("config.json")
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let system = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into());
        for letter in b'C'..=b'Z' {
            let drive = format!("{}:", letter as char);
            if drive.eq_ignore_ascii_case(&system) {
                continue;
            }
            let root = PathBuf::from(format!("{drive}/"));
            if root.is_dir() {
                return root.join("AnyFindData");
            }
        }
    }
    config_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf()
}

fn configured_data_dir() -> PathBuf {
    load_config()
        .get("data_dir")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir)
}

fn load_config() -> Value {
    let data_dir = default_data_dir().to_string_lossy().to_string();
    let mut cfg = json!({
        "hotkey": DEFAULT_HOTKEY,
        "kernel_url": DEFAULT_KERNEL,
        "autostart": false,
        "mock": false,
        "data_dir": data_dir,
    });
    if let Ok(text) = fs::read_to_string(config_path()) {
        if let Ok(saved) = serde_json::from_str::<Value>(&text) {
            if let (Some(base), Some(over)) = (cfg.as_object_mut(), saved.as_object()) {
                for (k, v) in over {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
    }
    cfg
}

fn save_config(cfg: &Value) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, serde_json::to_string_pretty(cfg).unwrap()).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_config() -> Value {
    load_config()
}

#[tauri::command]
fn save_config_cmd(cfg: Value) -> Result<Value, String> {
    save_config(&cfg)?;
    Ok(cfg)
}

/* ============ 内核 HTTP 代理 ============ */

fn kernel_base() -> String {
    let cfg = load_config();
    cfg.get("kernel_url")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_KERNEL)
        .trim_end_matches('/')
        .to_string()
}

/// 只自动管理 loopback 内核；兼容旧配置中的远程地址，但不干预它。
fn local_kernel_port(base: &str) -> Option<u16> {
    base.strip_prefix("http://127.0.0.1:")
        .or_else(|| base.strip_prefix("http://localhost:"))
        .and_then(|port| port.parse().ok())
}

fn persist_kernel_base(base: &str) -> Result<(), String> {
    let mut cfg = load_config();
    cfg["kernel_url"] = json!(base);
    save_config(&cfg)
}

fn bundled_kernel_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join("anyfindd.exe");
            if sidecar.is_file() {
                return Some(sidecar);
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../core/target/release/anyfindd.exe");
        if dev.is_file() {
            return Some(dev);
        }
    }
    None
}

/// 优先沿用上次端口；被占用时让操作系统分配空闲端口。
fn available_kernel_port(preferred: u16) -> Result<u16, String> {
    use std::net::TcpListener;
    if preferred != 0 && TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        return Ok(preferred);
    }
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| format!("无法分配本地内核端口: {e}"))
}

fn spawn_local_kernel(port: u16) -> Result<std::process::Child, String> {
    let exe = bundled_kernel_path().ok_or_else(|| "未找到随包 anyfindd.exe".to_string())?;
    let db = configured_data_dir().join("index.db");
    if let Some(dir) = db.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("无法创建索引目录: {e}"))?;
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--port")
        .arg(port.to_string())
        .arg("--db")
        .arg(db)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("anyfindd 启动失败: {e}"))
}

/// 不能只判断端口有 HTTP 响应：8790 也可能属于其他程序。
/// 同时校验 AnyFind 健康结构和预期索引路径，避免误连或重复启动。
fn kernel_uses_db(base: &str, expected: &std::path::Path) -> bool {
    let expected = expected.to_string_lossy().to_lowercase().replace('/', "\\");
    ureq::get(&format!("{base}/api/health"))
        .timeout(std::time::Duration::from_secs(5))
        .call()
        .ok()
        .and_then(|resp| resp.into_json::<Value>().ok())
        .and_then(|v| {
            let ok = v.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false);
            v.get("db_path")
                .and_then(|p| p.as_str())
                .map(|p| (ok, p.to_string()))
        })
        .is_some_and(|(ok, actual)| ok && actual.to_lowercase().replace('/', "\\") == expected)
}

fn wait_for_kernel(base: &str, expected: &std::path::Path) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if kernel_uses_db(base, expected) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn stop_local_kernel() -> Result<(), String> {
    use sysinfo::System;
    let base = kernel_base();
    let port = local_kernel_port(&base).ok_or_else(|| "远程内核不由桌面端管理".to_string())?;
    let port = port.to_string();
    let mut system = System::new();
    system.refresh_processes();
    let pids: Vec<sysinfo::Pid> = system
        .processes()
        .iter()
        .filter(|(_, p)| {
            p.name().eq_ignore_ascii_case("anyfindd.exe")
                && p.cmd()
                    .windows(2)
                    .any(|args| args[0] == "--port" && args[1] == port)
        })
        .map(|(pid, _)| *pid)
        .collect();
    for pid in pids {
        if let Some(process) = system.process(pid) {
            process.kill();
        }
    }
    for _ in 0..40 {
        if ureq::get(&format!("{base}/api/health"))
            .timeout(std::time::Duration::from_millis(250))
            .call()
            .is_err()
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("无法停止当前本地内核，未迁移任何文件".into())
}

/// 启动一次本地内核。端口冲突时换用空闲端口并写入内部配置；
/// 若另一个实例刚好先启动成功，则直接复用，不重复拉起。
fn start_local_kernel(configured: &str, expected: &std::path::Path) -> bool {
    let preferred = local_kernel_port(configured).unwrap_or(8790);
    for _ in 0..3 {
        if kernel_uses_db(configured, expected) {
            return true;
        }
        let port = match available_kernel_port(preferred) {
            Ok(port) => port,
            Err(e) => {
                eprintln!("[anyfind] {e}");
                return false;
            }
        };
        let base = format!("http://127.0.0.1:{port}");
        match spawn_local_kernel(port) {
            Ok(mut child) if wait_for_kernel(&base, expected) => {
                if let Err(e) = persist_kernel_base(&base) {
                    eprintln!("[anyfind] 保存自动内核端口失败: {e}");
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                eprintln!("[anyfind] 已启动随包 anyfindd（{port}）");
                drop(child);
                return true;
            }
            Ok(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(e) => eprintln!("[anyfind] {e}"),
        }
    }
    eprintln!("[anyfind] 本地内核启动失败：端口冲突重试后仍不可用");
    false
}

/// 在单实例插件生效后启动监督线程；内核异常退出时自动恢复。
/// 数据目录迁移期间暂停监督，避免把刚停止的旧内核重新拉起。
fn ensure_local_kernel() {
    std::thread::spawn(move || loop {
        if KERNEL_MIGRATING.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        let configured = kernel_base();
        if local_kernel_port(&configured).is_some() {
            let expected = configured_data_dir().join("index.db");
            if !kernel_uses_db(&configured, &expected) {
                start_local_kernel(&configured, &expected);
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
}

#[tauri::command]
fn choose_data_dir(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

fn copy_index_files(old: &std::path::Path, new: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    if !old.is_dir() {
        return Ok(Vec::new());
    }
    let mut created = Vec::new();
    let mut sources = Vec::new();
    for name in ["index.db", "index.db-wal", "index.db-shm", "names.bin"] {
        let source = old.join(name);
        if !source.is_file() {
            continue;
        }
        let dest = new.join(name);
        if dest.exists() {
            for path in created {
                let _ = fs::remove_file(path);
            }
            return Err(format!("目标目录已存在 {name}，请选择空的索引目录"));
        }
        if let Err(e) = fs::copy(&source, &dest) {
            for path in created {
                let _ = fs::remove_file(path);
            }
            return Err(format!("复制 {name} 失败: {e}"));
        }
        let source_len = fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        let dest_len = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if source_len != dest_len {
            for path in created {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_file(&dest);
            return Err(format!("{name} 复制校验失败，未删除原索引"));
        }
        created.push(dest);
        sources.push(source);
    }
    Ok(sources)
}

#[cfg(test)]
mod storage_tests {
    use super::{available_kernel_port, copy_index_files, local_kernel_port};
    use std::fs;

    #[test]
    fn local_kernel_port_only_accepts_loopback_http() {
        assert_eq!(local_kernel_port("http://127.0.0.1:8790"), Some(8790));
        assert_eq!(local_kernel_port("http://localhost:19321"), Some(19321));
        assert_eq!(local_kernel_port("http://192.168.1.2:8790"), None);
        assert_eq!(local_kernel_port("https://127.0.0.1:8790"), None);
    }

    #[test]
    fn occupied_preferred_port_falls_back_to_os_assigned_port() {
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let preferred = occupied.local_addr().unwrap().port();
        let selected = available_kernel_port(preferred).unwrap();
        assert_ne!(selected, preferred);
        assert!(std::net::TcpListener::bind(("127.0.0.1", selected)).is_ok());
    }

    #[test]
    fn index_copy_preserves_sources_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!(
            "anyfind-storage-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("index.db"), b"index-data").unwrap();
        fs::write(old.join("names.bin"), b"names-data").unwrap();

        let sources = copy_index_files(&old, &new).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(fs::read(new.join("index.db")).unwrap(), b"index-data");
        assert_eq!(fs::read(old.join("index.db")).unwrap(), b"index-data");

        fs::write(new.join("index.db-wal"), b"existing").unwrap();
        assert!(copy_index_files(&old, &new).is_err());
        assert_eq!(fs::read(new.join("index.db-wal")).unwrap(), b"existing");
        assert_eq!(fs::read(old.join("index.db")).unwrap(), b"index-data");
        fs::remove_dir_all(root).unwrap();
    }
}

fn set_data_dir_blocking(path: String) -> Result<Value, String> {
    let base = kernel_base();
    let Some(port) = local_kernel_port(&base) else {
        return Err("自定义内核的存储位置请在该内核机器上配置".into());
    };
    let new = PathBuf::from(path.trim());
    if !new.is_absolute() {
        return Err("索引目录必须是绝对路径".into());
    }
    fs::create_dir_all(&new).map_err(|e| format!("无法创建索引目录: {e}"))?;
    let old_cfg = load_config();
    let saved_dir = fs::read_to_string(config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("data_dir")
                .and_then(|d| d.as_str())
                .map(PathBuf::from)
        });
    let old = if let Some(saved) = saved_dir {
        saved
    } else {
        let legacy = config_path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        if legacy.join("index.db").is_file() {
            legacy
        } else {
            default_data_dir()
        }
    };
    if old == new {
        return Ok(json!({"ok": true, "data_dir": new.to_string_lossy()}));
    }
    let _migration = KernelMigrationGuard::new();
    stop_local_kernel()?;
    let old_files = match copy_index_files(&old, &new) {
        Ok(files) => files,
        Err(e) => {
            let _ = spawn_local_kernel(port);
            return Err(e);
        }
    };
    let mut cfg = old_cfg.clone();
    cfg["data_dir"] = json!(new.to_string_lossy());
    if let Err(e) = save_config(&cfg) {
        let _ = spawn_local_kernel(port);
        return Err(e);
    }
    let expected = new.join("index.db");
    if spawn_local_kernel(port).is_ok() && wait_for_kernel(&base, &expected) {
        for path in old_files {
            let _ = fs::remove_file(path);
        }
        return Ok(
            json!({"ok": true, "data_dir": new.to_string_lossy(), "db_path": expected.to_string_lossy()}),
        );
    }
    let _ = save_config(&old_cfg);
    let _ = stop_local_kernel();
    let _ = spawn_local_kernel(port);
    Err("新目录的内核验证失败，已恢复原配置；旧索引未删除".into())
}

#[tauri::command]
async fn set_data_dir(path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || set_data_dir_blocking(path))
        .await
        .map_err(|e| e.to_string())?
}

fn http_get(path: &str) -> Result<String, String> {
    let url = format!("{}{}", kernel_base(), path);
    let t0 = std::time::Instant::now();
    eprintln!("[kernel_get] → {}", url);
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(|e| {
            eprintln!("[kernel_get] ✗ {} ({:?}): {}", url, t0.elapsed(), e);
            format!("内核请求失败: {}", e)
        })?;
    let body = resp.into_string().map_err(|e| e.to_string())?;
    eprintln!(
        "[kernel_get] ✓ {} ({:?}, {} bytes)",
        url,
        t0.elapsed(),
        body.len()
    );
    Ok(body)
}

fn http_post(path: &str, body: Value) -> Result<String, String> {
    let url = format!("{}{}", kernel_base(), path);
    let t0 = std::time::Instant::now();
    eprintln!("[kernel_post] → {}", url);
    let resp = ureq::post(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send_json(body)
        .map_err(|e| {
            eprintln!("[kernel_post] ✗ {} ({:?}): {}", url, t0.elapsed(), e);
            format!("内核请求失败: {}", e)
        })?;
    let body = resp.into_string().map_err(|e| e.to_string())?;
    eprintln!(
        "[kernel_post] ✓ {} ({:?}, {} bytes)",
        url,
        t0.elapsed(),
        body.len()
    );
    Ok(body)
}

#[tauri::command]
async fn kernel_get(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || http_get(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn kernel_post(path: String, body: Value) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || http_post(&path, body))
        .await
        .map_err(|e| e.to_string())?
}

/* ============ 文件动作 ============ */

#[tauri::command]
fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(&path, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_in_folder(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if p.exists() {
        std::process::Command::new("explorer")
            .arg(format!(
                "/select,{}",
                p.to_string_lossy().replace('/', "\\")
            ))
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if let Some(parent) = p.parent() {
        if parent.is_dir() {
            std::process::Command::new("explorer")
                .arg(parent.to_string_lossy().replace('/', "\\"))
                .spawn()
                .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }
    Err("路径不存在".into())
}

/* ============ 全局热键 ============ */

/// "ctrl+shift+f" → "Control+Shift+F"（tauri Shortcut 解析格式）
fn normalize_shortcut(combo: &str) -> Result<String, String> {
    let mut mods: Vec<String> = Vec::new();
    let mut key: Option<String> = None;
    for part in combo.trim().to_lowercase().split('+') {
        match part.trim() {
            "ctrl" | "control" => mods.push("Control".into()),
            "shift" => mods.push("Shift".into()),
            "alt" => mods.push("Alt".into()),
            "win" | "meta" | "super" | "cmd" => mods.push("Super".into()),
            "" => {}
            k => {
                let nk = match k {
                    " " | "space" => "Space".into(),
                    "esc" => "Escape".into(),
                    "enter" | "return" => "Enter".into(),
                    "up" => "ArrowUp".into(),
                    "down" => "ArrowDown".into(),
                    "left" => "ArrowLeft".into(),
                    "right" => "ArrowRight".into(),
                    other => {
                        let mut c = other.chars();
                        match c.next() {
                            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                            None => return Err("空按键".into()),
                        }
                    }
                };
                key = Some(nk);
            }
        }
    }
    let key = key.ok_or_else(|| "无效组合键：缺少主键".to_string())?;
    if key == "Control" || key == "Shift" || key == "Alt" || key == "Super" {
        return Err("无效组合键：只有修饰键".into());
    }
    let mut all = mods;
    all.push(key);
    Ok(all.join("+"))
}

/// 前台锁绕过（沿用 filefind/desktop.py 验证过的方案）：
/// 先注入一次 Alt 按键让本进程获得前台权限，再 SetForegroundWindow。
/// 不用 always_on_top 弹跳 —— 它在 Windows 上会补回原生标题栏，
/// 且与 WebView2 输入处理并发时有死锁风险。
#[cfg(windows)]
fn force_foreground(window: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::keybd_event;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    if let Ok(hwnd) = window.hwnd() {
        let hwnd = hwnd.0 as _;
        const VK_MENU: u8 = 0x12;
        const KEYEVENTF_KEYUP: u32 = 0x2;
        unsafe {
            ShowWindow(hwnd, SW_RESTORE);
            keybd_event(VK_MENU, 0, 0, 0);
            keybd_event(VK_MENU, 0, KEYEVENTF_KEYUP, 0);
            SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(windows))]
fn force_foreground(_window: &tauri::WebviewWindow) {}

fn show_and_focus(window: &tauri::WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    force_foreground(window);
    let _ = window.set_focus();
    let _ = window.emit("focus-search", ());
}

fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);
        let minimized = window.is_minimized().unwrap_or(false);
        if visible && !minimized {
            let _ = window.hide();
        } else {
            show_and_focus(&window);
        }
    }
}

fn register_hotkey(app: &AppHandle, combo: &str) -> Result<(), String> {
    let shortcut = normalize_shortcut(combo)?;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    gs.register(shortcut.as_str())
        .map_err(|e| format!("热键注册失败（可能被占用）: {}", e))
}

#[tauri::command]
fn set_hotkey(app: AppHandle, combo: String) -> Result<Value, String> {
    let combo = combo.trim().to_lowercase();
    register_hotkey(&app, &combo)?;
    let mut cfg = load_config();
    cfg["hotkey"] = json!(combo);
    save_config(&cfg)?;
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut h) = state.hotkey.lock() {
            *h = combo.clone();
        }
    }
    Ok(json!({"ok": true, "hotkey": combo}))
}

/* ============ 开机自启 ============ */

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<Value, String> {
    use tauri_plugin_autostart::ManagerExt;
    let m = app.autolaunch();
    let r = if enabled { m.enable() } else { m.disable() };
    r.map_err(|e| e.to_string())?;
    let mut cfg = load_config();
    cfg["autostart"] = json!(enabled);
    save_config(&cfg)?;
    Ok(json!({"ok": true, "autostart": enabled}))
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

/* ============ 剪贴板（navigator.clipboard 无手势/无焦点时的兜底） ============ */

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text))
        .map_err(|e| e.to_string())
}

/* ============ 入口 ============ */

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = load_config();
    let hotkey = cfg
        .get("hotkey")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_HOTKEY)
        .to_string();
    let autostart = cfg
        .get("autostart")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    tauri::Builder::default()
        // 单实例必须在其他插件之前注册
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                show_and_focus(&window);
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_window(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            hotkey: Mutex::new(hotkey.clone()),
        })
        .setup(move |app| {
            ensure_local_kernel();

            // ── 托盘 ──
            let show = MenuItemBuilder::with_id("show", "显示 AnyFind").build(app)?;
            let hide = MenuItemBuilder::with_id("hide", "隐藏").build(app)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItemBuilder::with_id("quit", "退出").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&show)
                .item(&hide)
                .item(&sep)
                .item(&quit)
                .build()?;
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("AnyFind — 统一即时搜索")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            show_and_focus(&w);
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // ── 全局热键（配置值；失败仅日志，不阻塞启动） ──
            if let Err(e) = register_hotkey(app.handle(), &hotkey) {
                eprintln!("[anyfind] 全局热键 {} 注册失败: {}", hotkey, e);
            }

            // ── 开机自启与配置对齐 ──
            {
                use tauri_plugin_autostart::ManagerExt;
                let m = app.handle().autolaunch();
                let cur = m.is_enabled().unwrap_or(false);
                if cur != autostart {
                    let _ = if autostart { m.enable() } else { m.disable() };
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭按钮 = 隐藏到托盘，热键可唤起
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config_cmd,
            choose_data_dir,
            set_data_dir,
            kernel_get,
            kernel_post,
            open_path,
            reveal_in_folder,
            set_hotkey,
            set_autostart,
            get_autostart,
            copy_text,
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running anyfind desktop");
}
