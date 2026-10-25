// AnyFind desktop shell — Tauri v2.
// 壳职责：窗口/全局热键/托盘/单实例/开机自启/配置落盘，以及到 anyfindd 内核的
// HTTP 代理（绕过 webview CORS，Python/Rust 内核同契约无缝切换）。

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const DEFAULT_HOTKEY: &str = "ctrl+shift+f";
const DEFAULT_KERNEL: &str = "http://127.0.0.1:8790";

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

fn load_config() -> Value {
    let mut cfg = json!({
        "hotkey": DEFAULT_HOTKEY,
        "kernel_url": DEFAULT_KERNEL,
        "autostart": false,
        "mock": false,
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

/// 默认本地内核未运行时，启动安装包内置 sidecar；自定义远程地址不干预。
fn ensure_local_kernel() {
    if kernel_base() != DEFAULT_KERNEL {
        return;
    }
    std::thread::spawn(|| {
        if ureq::get(&format!("{DEFAULT_KERNEL}/api/health"))
            .timeout(std::time::Duration::from_millis(800))
            .call()
            .is_ok()
        {
            return;
        }
        let Some(exe) = bundled_kernel_path() else {
            eprintln!("[anyfind] 未找到随包 anyfindd.exe");
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--port")
            .arg("8790")
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
        match cmd.spawn() {
            Ok(_) => eprintln!("[anyfind] 已启动随包 anyfindd（8790）"),
            Err(e) => eprintln!("[anyfind] anyfindd 启动失败: {e}"),
        }
    });
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
    eprintln!("[kernel_get] ✓ {} ({:?}, {} bytes)", url, t0.elapsed(), body.len());
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
    eprintln!("[kernel_post] ✓ {} ({:?}, {} bytes)", url, t0.elapsed(), body.len());
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
            .arg(format!("/select,{}", p.to_string_lossy().replace('/', "\\")))
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
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_RESTORE};
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
    ensure_local_kernel();
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
        .plugin(tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    toggle_window(app);
                }
            })
            .build())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            hotkey: Mutex::new(hotkey.clone()),
        })
        .setup(move |app| {
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
