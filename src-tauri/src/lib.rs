use std::{
    collections::hash_map::DefaultHasher,
    collections::HashMap,
    fs,
    process::Command,
    env,
    path::{Path, PathBuf},
    thread,
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use futures_util::{SinkExt, StreamExt};
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sysinfo::System;
use tauri::{
    menu::{MenuBuilder, MenuEvent, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WindowEvent,
};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::mpsc::{self, UnboundedSender},
    time::{self, Duration},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use tauri_plugin_opener::OpenerExt;

const DEFAULT_PORT: u16 = 39393;
const UDP_MOVE_PORT: u16 = 39394;
const OPEN_APPS_MAX: usize = 6;
const MIN_POINTER_SPEED: usize = 5;
const MAX_POINTER_SPEED: usize = 50;
const DEFAULT_POINTER_SPEED: usize = 25;
static WS_MOVE_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);
static UDP_MOVE_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

fn detect_host_os_profile() -> &'static str {
    match std::env::consts::OS {
        "macos" => "mac",
        "windows" => "windows",
        _ => "windows",
    }
}

fn generate_quick_code() -> String {
    let mut rng = rand::rng();
    let code: u16 = rng.random_range(0..10000);
    format!("{code:04}")
}

fn host_display_name() -> String {
    env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Wireless PC Host".to_string())
}

#[cfg(target_os = "windows")]
fn host_model_name() -> String {
    if let Ok(output) = Command::new("wmic")
        .args(["computersystem", "get", "model"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let model = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.eq_ignore_ascii_case("model"))
                .next()
                .unwrap_or("Windows PC");
            return model.to_string();
        }
    }
    "Windows PC".to_string()
}

#[cfg(not(target_os = "windows"))]
fn host_model_name() -> String {
    "Desktop".to_string()
}

fn host_device_id() -> String {
    let name = host_display_name();
    let os = detect_host_os_profile();
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    os.hash(&mut hasher);
    let v = hasher.finish();
    format!("mp-{:016x}", v)
}

fn announce_mdns(current_token: &str, requires_token: bool) -> Result<ServiceDaemon, String> {
    let mdns = ServiceDaemon::new().map_err(|e| format!("mdns init failed: {e}"))?;
    let ip = local_ip_address::local_ip()
        .map(|i| i.to_string())
        .map_err(|e| format!("mdns ip detect failed: {e}"))?;

    let service_type = "_wirelesspc._tcp.local.";
    let instance_name = host_display_name();
    let host_name = "wirelesspc.local.";
    let mut txt = HashMap::<String, String>::new();
    txt.insert("app".to_string(), "wirelesspc".to_string());
    txt.insert("id".to_string(), host_device_id());
    txt.insert(
        "requires_token".to_string(),
        (requires_token && !current_token.is_empty()).to_string(),
    );
    txt.insert("os_profile".to_string(), detect_host_os_profile().to_string());
    txt.insert("port".to_string(), DEFAULT_PORT.to_string());
    txt.insert("udp_port".to_string(), UDP_MOVE_PORT.to_string());

    let info = ServiceInfo::new(
        service_type,
        instance_name.as_str(),
        host_name,
        ip.as_str(),
        DEFAULT_PORT,
        Some(txt),
    )
    .map_err(|e| format!("mdns service info failed: {e}"))?;

    mdns.register(info)
        .map_err(|e| format!("mdns register failed: {e}"))?;

    Ok(mdns)
}

#[derive(Clone)]
struct AppState {
    token: Arc<Mutex<String>>,
    connections: Arc<AtomicUsize>,
    pointer_speed: Arc<AtomicUsize>,
    power_actions_enabled: Arc<AtomicUsize>,
    connection_code_enabled: Arc<AtomicUsize>,
    connected_devices: Arc<Mutex<HashMap<String, ConnectedDeviceEntry>>>,
    last_connected_device: Arc<Mutex<String>>,
    started_at: Arc<Instant>,
    bytes_in: Arc<AtomicUsize>,
    input_tx: UnboundedSender<InputCommand>,
    open_apps: Arc<Mutex<OpenAppsStore>>,
}

#[derive(Serialize)]
struct PairingInfo {
    host: String,
    port: u16,
    token: String,
    ws_url: String,
    device_name: String,
    device_model: String,
}

#[derive(Clone, Serialize)]
struct ConnectionState {
    connections: usize,
    connected_to: String,
    devices: Vec<ConnectedDeviceEntry>,
}

#[derive(Clone, Serialize)]
struct ConnectedDeviceEntry {
    id: String,
    name: String,
    model: String,
    ip: String,
    times: usize,
    secured: bool,
    password_required: bool,
    active_sessions: usize,
}

#[derive(Clone, Serialize)]
struct EventPayload {
    message: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct OpenAppEntry {
    id: String,
    name: String,
    path: String,
    icon_path: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct OpenAppsStore {
    items: Vec<OpenAppEntry>,
    updated_at: u64,
}

#[derive(Clone, Serialize)]
struct OpenAppClientEntry {
    id: String,
    name: String,
    path: String,
    icon_data_url: Option<String>,
}

#[derive(Deserialize)]
struct AuthMessage {
    #[serde(rename = "type")]
    kind: String,
    token: String,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    device_model: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ControlEvent {
    #[serde(rename = "ping")]
    Ping {
        #[serde(default)]
        ts: Option<u64>,
    },
    #[serde(rename = "move")]
    Move { dx: f64, dy: f64 },
    #[serde(rename = "pointer_speed")]
    PointerSpeed { value: i32 },
    #[serde(rename = "click")]
    Click {
        button: String,
        #[serde(default)]
        state: Option<String>,
    },
    #[serde(rename = "scroll")]
    Scroll {
        dy: f64,
        #[serde(default)]
        dx: Option<f64>,
    },
    #[serde(rename = "key")]
    Key { key: String },
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "shortcut")]
    Shortcut { combo: String },
    #[serde(rename = "media")]
    Media { action: String },
    #[serde(rename = "system")]
    System { action: String },
    #[serde(rename = "clipboard_set")]
    ClipboardSet { text: String },
    #[serde(rename = "clipboard_get")]
    ClipboardGet,
    #[serde(rename = "open_apps_get")]
    OpenAppsGet,
    #[serde(rename = "open_app_open")]
    OpenAppOpen { id: String },
    #[serde(rename = "open_app_add")]
    OpenAppAdd { path: String },
    #[serde(rename = "open_app_remove")]
    OpenAppRemove { id: String },
    #[serde(rename = "open_apps_sync")]
    OpenAppsSync {
        items: Vec<OpenAppSyncEntry>,
        updated_at: u64,
    },
}

#[derive(Clone, Deserialize)]
struct OpenAppSyncEntry {
    id: Option<String>,
    name: String,
    path: String,
}

#[derive(Clone)]
enum InputCommand {
    Move { dx: f64, dy: f64 },
    Click { button: String, state: Option<String> },
    Scroll { dy: f64, dx: f64 },
    Key { key: String },
    Text { text: String },
    Shortcut { combo: String },
    Media { action: String },
    System { action: String },
}

fn press_combo(enigo: &mut Enigo, keys: &[Key], final_key: Key) {
    for k in keys {
        let _ = enigo.key(*k, Direction::Press);
    }
    let _ = enigo.key(final_key, Direction::Click);
    for k in keys.iter().rev() {
        let _ = enigo.key(*k, Direction::Release);
    }
}

fn split_shortcut_tokens(combo: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in combo.chars() {
        if ch == '+' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                tokens.push(trimmed.to_string());
            } else {
                // Treat repeated '+' as the plus key token, e.g. CTRL++.
                tokens.push("+".to_string());
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        tokens.push(trimmed.to_string());
    }
    tokens
}

fn parse_modifier_token(token: &str) -> Option<Key> {
    match token.trim().to_uppercase().as_str() {
        "CTRL" | "CONTROL" => Some(Key::Control),
        "ALT" | "OPTION" => Some(Key::Alt),
        "SHIFT" => Some(Key::Shift),
        "WIN" | "WINDOWS" | "META" | "CMD" | "COMMAND" | "SUPER" => Some(Key::Meta),
        _ => None,
    }
}

fn parse_function_key(token_upper: &str) -> Option<Key> {
    let number = token_upper.strip_prefix('F')?.parse::<u8>().ok()?;
    match number {
        1 => Some(Key::F1),
        2 => Some(Key::F2),
        3 => Some(Key::F3),
        4 => Some(Key::F4),
        5 => Some(Key::F5),
        6 => Some(Key::F6),
        7 => Some(Key::F7),
        8 => Some(Key::F8),
        9 => Some(Key::F9),
        10 => Some(Key::F10),
        11 => Some(Key::F11),
        12 => Some(Key::F12),
        13 => Some(Key::F13),
        14 => Some(Key::F14),
        15 => Some(Key::F15),
        16 => Some(Key::F16),
        17 => Some(Key::F17),
        18 => Some(Key::F18),
        19 => Some(Key::F19),
        20 => Some(Key::F20),
        21 => Some(Key::F21),
        22 => Some(Key::F22),
        23 => Some(Key::F23),
        24 => Some(Key::F24),
        _ => None,
    }
}

fn parse_key_token(token: &str) -> Option<Key> {
    let upper = token.trim().to_uppercase();
    let key = match upper.as_str() {
        "ESC" | "ESCAPE" => Some(Key::Escape),
        "TAB" => Some(Key::Tab),
        "ENTER" | "RETURN" => Some(Key::Return),
        "BACKSPACE" | "BKSP" => Some(Key::Backspace),
        "SPACE" | "SPACEBAR" => Some(Key::Space),
        "DEL" | "DELETE" => Some(Key::Delete),
        "INS" | "INSERT" => Some(Key::Insert),
        "HOME" => Some(Key::Home),
        "END" => Some(Key::End),
        "PGUP" | "PAGEUP" => Some(Key::PageUp),
        "PGDN" | "PAGEDOWN" => Some(Key::PageDown),
        "UP" | "UPARROW" => Some(Key::UpArrow),
        "DOWN" | "DOWNARROW" => Some(Key::DownArrow),
        "LEFT" | "LEFTARROW" => Some(Key::LeftArrow),
        "RIGHT" | "RIGHTARROW" => Some(Key::RightArrow),
        "PRTSC" | "PRINTSCREEN" => Some(Key::Print),
        "PLUS" | "+" | "=" => Some(Key::Unicode('=')),
        "MINUS" | "DASH" | "-" | "_" => Some(Key::Unicode('-')),
        "COMMA" => Some(Key::Unicode(',')),
        "PERIOD" | "DOT" => Some(Key::Unicode('.')),
        "SLASH" => Some(Key::Unicode('/')),
        "BACKSLASH" => Some(Key::Unicode('\\')),
        "SEMICOLON" => Some(Key::Unicode(';')),
        "QUOTE" | "APOSTROPHE" => Some(Key::Unicode('\'')),
        "GRAVE" | "BACKTICK" => Some(Key::Unicode('`')),
        _ => parse_function_key(&upper),
    };
    if key.is_some() {
        return key;
    }

    let mut chars = upper.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(Key::Unicode(first.to_ascii_lowercase()));
    }
    None
}

fn parse_shortcut_combo(combo: &str) -> Option<(Vec<Key>, Key)> {
    let tokens = split_shortcut_tokens(combo);
    if tokens.is_empty() {
        return None;
    }

    let mut modifiers = Vec::new();
    let mut final_key: Option<Key> = None;

    for token in tokens {
        if let Some(modifier) = parse_modifier_token(&token) {
            if final_key.is_some() {
                return None;
            }
            if !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
            continue;
        }
        if final_key.is_some() {
            return None;
        }
        final_key = parse_key_token(&token);
        if final_key.is_none() {
            return None;
        }
    }

    final_key.map(|key| (modifiers, key))
}

fn handle_shortcut(enigo: &mut Enigo, combo: &str) {
    let normalized = combo.trim().to_uppercase();
    if normalized == "CTRL+ALT+DEL" || normalized == "CTRL+ALT+DELETE" {
        // Blocked by OS security boundaries; keep compatibility without crashing.
        return;
    }

    if let Some((modifiers, final_key)) = parse_shortcut_combo(combo) {
        press_combo(enigo, &modifiers, final_key);
    }
}

fn handle_media(enigo: &mut Enigo, action: &str) {
    let key = match action.trim().to_uppercase().as_str() {
        "MEDIA_TOGGLE" | "PLAY_PAUSE" => Some(Key::MediaPlayPause),
        "MEDIA_NEXT" | "NEXT" => Some(Key::MediaNextTrack),
        "MEDIA_PREV" | "PREVIOUS" => Some(Key::MediaPrevTrack),
        "MEDIA_STOP" | "STOP" => Some(Key::MediaStop),
        "VOLUME_UP" => Some(Key::VolumeUp),
        "VOLUME_DOWN" => Some(Key::VolumeDown),
        "VOLUME_MUTE" | "MUTE" => Some(Key::VolumeMute),
        _ => None,
    };
    if let Some(k) = key {
        let _ = enigo.key(k, Direction::Click);
    }
}

#[cfg(target_os = "windows")]
fn run_windows_command(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args).spawn();
}

fn handle_system(action: &str) {
    #[cfg(target_os = "windows")]
    {
        match action.trim().to_uppercase().as_str() {
            "SYSTEM_LOCK" | "LOCK" => {
                run_windows_command("rundll32.exe", &["user32.dll,LockWorkStation"]);
            }
            "SYSTEM_SLEEP" | "SLEEP" => {
                run_windows_command("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]);
            }
            "BRIGHTNESS_UP" => {
                run_windows_command(
                    "powershell",
                    &[
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods | Select-Object -First 1).WmiSetBrightness(1,[Math]::Min(100,((Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness | Select-Object -First 1).CurrentBrightness + 10)))",
                    ],
                );
            }
            "BRIGHTNESS_DOWN" => {
                run_windows_command(
                    "powershell",
                    &[
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods | Select-Object -First 1).WmiSetBrightness(1,[Math]::Max(0,((Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness | Select-Object -First 1).CurrentBrightness - 10)))",
                    ],
                );
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "windows")]
fn startup_registry_name() -> &'static str {
    "WirelessPCServer"
}

#[cfg(target_os = "windows")]
fn startup_registry_path() -> &'static str {
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
}

#[cfg(target_os = "windows")]
fn get_launch_on_startup_impl() -> bool {
    Command::new("reg")
        .args([
            "query",
            startup_registry_path(),
            "/v",
            startup_registry_name(),
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn get_launch_on_startup_impl() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn set_launch_on_startup_impl(enabled: bool) -> bool {
    if enabled {
        let exe = match std::env::current_exe() {
            Ok(v) => v,
            Err(_) => return false,
        };
        let exe_str = match exe.to_str() {
            Some(v) => format!("\"{}\"", v.replace('"', "")),
            None => return false,
        };
        Command::new("reg")
            .args([
                "add",
                startup_registry_path(),
                "/v",
                startup_registry_name(),
                "/t",
                "REG_SZ",
                "/d",
                &exe_str,
                "/f",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        Command::new("reg")
            .args([
                "delete",
                startup_registry_path(),
                "/v",
                startup_registry_name(),
                "/f",
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(not(target_os = "windows"))]
fn set_launch_on_startup_impl(_enabled: bool) -> bool {
    false
}

fn clipboard_set_text(text: &str) -> Result<(), String> {
    let mut last_err = String::new();
    for _ in 0..6 {
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.set_text(text.to_string()) {
                Ok(_) => return Ok(()),
                Err(e) => last_err = format!("clipboard set failed: {e}"),
            },
            Err(e) => last_err = format!("clipboard init failed: {e}"),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(last_err)
}

fn clipboard_get_text() -> Result<String, String> {
    let mut last_err = String::new();
    for _ in 0..6 {
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.get_text() {
                Ok(v) => return Ok(v),
                Err(e) => last_err = format!("clipboard get failed: {e}"),
            },
            Err(e) => last_err = format!("clipboard init failed: {e}"),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(last_err)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn open_apps_root_dir() -> PathBuf {
    let base = env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir());
    let dir = base.join("wirelesspc").join("open_apps");
    let _ = fs::create_dir_all(dir.join("icons"));
    dir
}

fn open_apps_store_path() -> PathBuf {
    open_apps_root_dir().join("store.json")
}

fn open_apps_icon_path(id: &str) -> PathBuf {
    open_apps_root_dir().join("icons").join(format!("{id}.png"))
}

fn persist_open_apps(store: &OpenAppsStore) {
    let path = open_apps_store_path();
    if let Ok(text) = serde_json::to_string_pretty(store) {
        let _ = fs::write(path, text);
    }
}

fn load_open_apps_store() -> OpenAppsStore {
    let path = open_apps_store_path();
    let raw = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return OpenAppsStore::default(),
    };
    let mut store = serde_json::from_str::<OpenAppsStore>(&raw).unwrap_or_default();
    if store.items.len() > OPEN_APPS_MAX {
        store.items.truncate(OPEN_APPS_MAX);
    }
    store
}

fn make_open_app_id(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_ascii_lowercase().hash(&mut hasher);
    format!("app-{:016x}", hasher.finish())
}

#[cfg(target_os = "windows")]
fn extract_icon_from_exe(exe_path: &Path, icon_path: &Path) -> bool {
    let exe = exe_path.to_string_lossy().replace('\'', "''");
    let out = icon_path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; \
         $i=[System.Drawing.Icon]::ExtractAssociatedIcon('{exe}'); \
         if($null -eq $i){{exit 1}}; \
         $b=$i.ToBitmap(); \
         $b.Save('{out}',[System.Drawing.Imaging.ImageFormat]::Png); \
         $b.Dispose(); $i.Dispose(); exit 0;"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn extract_icon_from_exe(_exe_path: &Path, _icon_path: &Path) -> bool {
    false
}

fn build_open_app_entry(path_raw: &str) -> Result<OpenAppEntry, String> {
    let trimmed = path_raw.trim();
    if trimmed.is_empty() {
        return Err("empty_path".to_string());
    }
    let normalized = PathBuf::from(trimmed);
    if !normalized.exists() {
        return Err("path_not_found".to_string());
    }
    #[cfg(target_os = "windows")]
    if normalized
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| !v.eq_ignore_ascii_case("exe"))
        .unwrap_or(true)
    {
        return Err("not_exe".to_string());
    }

    let full = normalized
        .canonicalize()
        .unwrap_or(normalized.clone())
        .to_string_lossy()
        .to_string();
    let id = make_open_app_id(&full);
    let name = normalized
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("App")
        .to_string();
    let icon_path = open_apps_icon_path(&id);
    let _ = extract_icon_from_exe(&normalized, &icon_path);
    Ok(OpenAppEntry {
        id,
        name,
        path: full,
        icon_path: icon_path.to_string_lossy().to_string(),
    })
}

fn open_apps_payload(store: &OpenAppsStore) -> Vec<OpenAppClientEntry> {
    store
        .items
        .iter()
        .map(|item| {
            let icon_data_url = fs::read(&item.icon_path).ok().map(|bytes| {
                let encoded = BASE64_STANDARD.encode(bytes);
                format!("data:image/png;base64,{encoded}")
            });
            OpenAppClientEntry {
                id: item.id.clone(),
                name: item.name.clone(),
                path: item.path.clone(),
                icon_data_url,
            }
        })
        .collect()
}

fn launch_open_app(path: &str) -> bool {
    Command::new(path)
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}

fn discover_common_open_apps() -> Vec<String> {
    let mut candidates = Vec::new();
    let pf = env::var("ProgramFiles").unwrap_or_default();
    let pfx86 = env::var("ProgramFiles(x86)").unwrap_or_default();
    let local = env::var("LOCALAPPDATA").unwrap_or_default();
    let add = |v: &mut Vec<String>, p: String| {
        if !p.is_empty() && Path::new(&p).exists() {
            v.push(p);
        }
    };
    add(&mut candidates, format!("{pf}\\Google\\Chrome\\Application\\chrome.exe"));
    add(&mut candidates, format!("{pf}\\Mozilla Firefox\\firefox.exe"));
    add(&mut candidates, format!("{pf}\\Microsoft\\Edge\\Application\\msedge.exe"));
    add(&mut candidates, format!("{local}\\Programs\\Microsoft VS Code\\Code.exe"));
    add(&mut candidates, format!("{local}\\Spotify\\Spotify.exe"));
    add(&mut candidates, format!("{local}\\Discord\\Update.exe"));
    add(&mut candidates, format!("{pf}\\VideoLAN\\VLC\\vlc.exe"));
    add(&mut candidates, format!("{pfx86}\\Steam\\steam.exe"));
    candidates
}

fn is_power_action(action: &str) -> bool {
    matches!(
        action.trim().to_uppercase().as_str(),
        "SYSTEM_LOCK" | "LOCK" | "SYSTEM_SLEEP" | "SLEEP" | "SYSTEM_RESTART" | "RESTART" | "SYSTEM_SHUTDOWN" | "SHUTDOWN"
    )
}

async fn run_input_worker(
    mut rx: mpsc::UnboundedReceiver<InputCommand>,
    pointer_speed: Arc<AtomicUsize>,
) {
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut pending_dx = 0.0_f64;
    let mut pending_dy = 0.0_f64;
    let mut ticker = time::interval(Duration::from_millis(4));
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    const MAX_STEP_PER_TICK: f64 = 22.0;

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                let Some(cmd) = maybe_cmd else { break };
                match cmd {
                    InputCommand::Move { dx, dy } => {
                        let speed = pointer_speed
                            .load(Ordering::Relaxed)
                            .clamp(MIN_POINTER_SPEED, MAX_POINTER_SPEED) as f64;
                        let factor = speed / 10.0;
                        pending_dx = (pending_dx + (dx * factor)).clamp(-2000.0, 2000.0);
                        pending_dy = (pending_dy + (dy * factor)).clamp(-2000.0, 2000.0);
                    }
                    InputCommand::Click { button, state } => {
                        let mapped = match button.as_str() {
                            "left" => Some(Button::Left),
                            "right" => Some(Button::Right),
                            "middle" => Some(Button::Middle),
                            _ => None,
                        };
                        if let Some(btn) = mapped {
                            let direction = match state
                                .as_deref()
                                .map(|v| v.trim().to_ascii_lowercase())
                                .as_deref()
                            {
                                Some("down") | Some("press") => Direction::Press,
                                Some("up") | Some("release") => Direction::Release,
                                _ => Direction::Click,
                            };
                            let _ = enigo.button(btn, direction);
                        }
                    }
                    InputCommand::Scroll { dy, dx } => {
                        let v_amount = dy.round() as i32;
                        let h_amount = dx.round() as i32;
                        if v_amount != 0 {
                            let _ = enigo.scroll(v_amount, enigo::Axis::Vertical);
                        }
                        if h_amount != 0 {
                            let _ = enigo.scroll(h_amount, enigo::Axis::Horizontal);
                        }
                    }
                    InputCommand::Key { key } => {
                        match key.to_uppercase().as_str() {
                            "ENTER" | "RETURN" => {
                                let _ = enigo.key(Key::Return, Direction::Click);
                            }
                            "BACKSPACE" => {
                                let _ = enigo.key(Key::Backspace, Direction::Click);
                            }
                            "TAB" => {
                                let _ = enigo.key(Key::Tab, Direction::Click);
                            }
                            "ESC" | "ESCAPE" => {
                                let _ = enigo.key(Key::Escape, Direction::Click);
                            }
                            "ARROW_UP" => {
                                let _ = enigo.key(Key::UpArrow, Direction::Click);
                            }
                            "ARROW_DOWN" => {
                                let _ = enigo.key(Key::DownArrow, Direction::Click);
                            }
                            "ARROW_LEFT" => {
                                let _ = enigo.key(Key::LeftArrow, Direction::Click);
                            }
                            "ARROW_RIGHT" => {
                                let _ = enigo.key(Key::RightArrow, Direction::Click);
                            }
                            "enter" => {
                                let _ = enigo.key(Key::Return, Direction::Click);
                            }
                            "backspace" => {
                                let _ = enigo.key(Key::Backspace, Direction::Click);
                            }
                            _ => {
                                let _ = enigo.text(&key);
                            }
                        }
                    }
                    InputCommand::Text { text } => {
                        let _ = enigo.text(&text);
                    }
                    InputCommand::Shortcut { combo } => {
                        handle_shortcut(&mut enigo, &combo);
                    }
                    InputCommand::Media { action } => {
                        handle_media(&mut enigo, &action);
                    }
                    InputCommand::System { action } => {
                        handle_system(&action);
                    }
                }
            }
            _ = ticker.tick() => {
                let dynamic_cap_x = (pending_dx.abs() * 0.35).clamp(2.0, MAX_STEP_PER_TICK);
                let dynamic_cap_y = (pending_dy.abs() * 0.35).clamp(2.0, MAX_STEP_PER_TICK);
                let sx = pending_dx.clamp(-dynamic_cap_x, dynamic_cap_x);
                let sy = pending_dy.clamp(-dynamic_cap_y, dynamic_cap_y);
                let mx = sx.trunc() as i32;
                let my = sy.trunc() as i32;

                if mx != 0 || my != 0 {
                    let _ = enigo.move_mouse(mx, my, Coordinate::Rel);
                    pending_dx -= mx as f64;
                    pending_dy -= my as f64;
                }
            }
        }
    }
}

#[tauri::command]
fn generate_pairing_info(state: tauri::State<AppState>) -> PairingInfo {
    let host = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let token = generate_quick_code();

    if let Ok(mut current) = state.token.lock() {
        *current = token.clone();
    }

    let ws_url = format!("ws://{}:{}?token={}", host, DEFAULT_PORT, token);
    PairingInfo {
        host,
        port: DEFAULT_PORT,
        token,
        ws_url,
        device_name: host_display_name(),
        device_model: host_model_name(),
    }
}

#[tauri::command]
fn set_power_actions_enabled(state: tauri::State<AppState>, enabled: bool) {
    state
        .power_actions_enabled
        .store(if enabled { 1 } else { 0 }, Ordering::Relaxed);
}

#[tauri::command]
fn get_power_actions_enabled(state: tauri::State<AppState>) -> bool {
    state.power_actions_enabled.load(Ordering::Relaxed) > 0
}

#[tauri::command]
fn set_connection_code_enabled(state: tauri::State<AppState>, enabled: bool) {
    state
        .connection_code_enabled
        .store(if enabled { 1 } else { 0 }, Ordering::Relaxed);
}

#[tauri::command]
fn get_connection_code_enabled(state: tauri::State<AppState>) -> bool {
    state.connection_code_enabled.load(Ordering::Relaxed) > 0
}

#[tauri::command]
fn get_launch_on_startup() -> bool {
    get_launch_on_startup_impl()
}

#[tauri::command]
fn set_launch_on_startup(enabled: bool) -> bool {
    set_launch_on_startup_impl(enabled)
}

#[tauri::command]
fn clear_connected_devices_history(state: tauri::State<AppState>, app: AppHandle) {
    if let Ok(mut map) = state.connected_devices.lock() {
        map.clear();
    }
    if let Ok(mut last) = state.last_connected_device.lock() {
        last.clear();
    }
    emit_connections(&app, &state);
}

#[tauri::command]
fn get_open_apps(state: tauri::State<AppState>) -> serde_json::Value {
    let store = state
        .open_apps
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    json!({
        "items": open_apps_payload(&store),
        "updated_at": store.updated_at
    })
}

#[tauri::command]
fn add_open_app(state: tauri::State<AppState>, path: String) -> Result<serde_json::Value, String> {
    let entry = build_open_app_entry(&path)?;
    let mut store = state
        .open_apps
        .lock()
        .map_err(|_| "store_lock_failed".to_string())?;
    if store.items.len() >= OPEN_APPS_MAX {
        return Err("max_apps_reached".to_string());
    }
    if store.items.iter().any(|i| i.path.eq_ignore_ascii_case(&entry.path)) {
        return Err("already_exists".to_string());
    }
    store.items.push(entry.clone());
    store.updated_at = now_unix_ms();
    persist_open_apps(&store);
    Ok(json!({
        "item": OpenAppClientEntry {
            id: entry.id,
            name: entry.name,
            path: entry.path,
            icon_data_url: fs::read(&entry.icon_path).ok().map(|bytes| format!("data:image/png;base64,{}", BASE64_STANDARD.encode(bytes)))
        },
        "updated_at": store.updated_at
    }))
}

#[tauri::command]
fn remove_open_app(state: tauri::State<AppState>, id: String) -> bool {
    let mut store = match state.open_apps.lock() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let before = store.items.len();
    store.items.retain(|i| i.id != id);
    if before == store.items.len() {
        return false;
    }
    let _ = fs::remove_file(open_apps_icon_path(&id));
    store.updated_at = now_unix_ms();
    persist_open_apps(&store);
    true
}

#[tauri::command]
fn discover_open_apps(state: tauri::State<AppState>) -> usize {
    let mut added = 0usize;
    let mut store = match state.open_apps.lock() {
        Ok(v) => v,
        Err(_) => return 0,
    };
    for path in discover_common_open_apps() {
        if store.items.len() >= OPEN_APPS_MAX {
            break;
        }
        let Ok(entry) = build_open_app_entry(&path) else {
            continue;
        };
        if store
            .items
            .iter()
            .any(|i| i.path.eq_ignore_ascii_case(&entry.path))
        {
            continue;
        }
        store.items.push(entry);
        added += 1;
    }
    if added > 0 {
        store.updated_at = now_unix_ms();
        persist_open_apps(&store);
    }
    added
}

#[tauri::command]
fn pick_open_app_path() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        return rfd::FileDialog::new()
            .add_filter("Executable", &["exe"])
            .pick_file()
            .map(|p| p.to_string_lossy().to_string());
    }
    #[allow(unreachable_code)]
    None
}

fn current_token(state: &AppState) -> String {
    state
        .token
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| String::new())
}

fn is_connection_code_enabled(state: &AppState) -> bool {
    state.connection_code_enabled.load(Ordering::Relaxed) > 0
}

fn is_token_authorized(state: &AppState, provided: &str) -> bool {
    if !is_connection_code_enabled(state) {
        return true;
    }
    let expected = current_token(state);
    !expected.is_empty() && provided == expected
}

fn register_device_connection(state: &AppState, auth: &AuthMessage, peer_ip: &str) -> String {
    let name = auth
        .device_name
        .clone()
        .unwrap_or_else(|| "Unknown Device".to_string());
    let model = auth
        .device_model
        .clone()
        .or_else(|| auth.platform.clone())
        .unwrap_or_else(|| "Unknown".to_string());
    let password_required = is_connection_code_enabled(state);
    let secured = password_required && !auth.token.trim().is_empty();
    let id = format!("{}@{}", name, peer_ip);

    if let Ok(mut map) = state.connected_devices.lock() {
        if let Some(existing) = map.get_mut(&id) {
            existing.times += 1;
            existing.active_sessions += 1;
            existing.model = model.clone();
            existing.password_required = password_required;
            existing.secured = secured;
        } else {
            map.insert(
                id.clone(),
                ConnectedDeviceEntry {
                    id: id.clone(),
                    name: name.clone(),
                    model: model.clone(),
                    ip: peer_ip.to_string(),
                    times: 1,
                    secured,
                    password_required,
                    active_sessions: 1,
                },
            );
        }
    }

    if let Ok(mut last) = state.last_connected_device.lock() {
        *last = name;
    }

    id
}

fn unregister_device_connection(state: &AppState, device_id: &str) {
    if let Ok(mut map) = state.connected_devices.lock() {
        if let Some(entry) = map.get_mut(device_id) {
            if entry.active_sessions > 0 {
                entry.active_sessions -= 1;
            }
        }
    }
}

fn emit_connections(app: &AppHandle, state: &AppState) {
    let devices = if let Ok(map) = state.connected_devices.lock() {
        let mut v: Vec<ConnectedDeviceEntry> = map.values().cloned().collect();
        v.sort_by(|a, b| b.times.cmp(&a.times));
        v
    } else {
        Vec::new()
    };

    let connected_to = if let Ok(last) = state.last_connected_device.lock() {
        last.clone()
    } else {
        String::new()
    };

    let _ = app.emit(
        "ws-connection-state",
        ConnectionState {
            connections: state.connections.load(Ordering::Relaxed),
            connected_to,
            devices,
        },
    );
}

async fn handle_socket(
    raw_stream: tokio::net::TcpStream,
    state: AppState,
    app: AppHandle,
) -> Result<(), String> {
    let peer_ip = raw_stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let mut ws = accept_async(raw_stream)
        .await
        .map_err(|e| format!("websocket handshake failed: {e}"))?;

    let first = ws
        .next()
        .await
        .ok_or_else(|| "client disconnected before auth".to_string())?
        .map_err(|e| format!("auth receive error: {e}"))?;

    let first_text = match first {
        Message::Text(txt) => txt.to_string(),
        _ => return Err("expected auth text message".into()),
    };

    let first_value: serde_json::Value =
        serde_json::from_str(&first_text).map_err(|_| "invalid first payload".to_string())?;
    let kind = first_value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if kind == "probe" {
        let nonce = first_value
            .get("nonce")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let _ = ws
            .send(Message::Text(
                json!({
                    "type": "probe",
                    "app": "wirelesspc",
                    "nonce": nonce,
                    "id": host_device_id(),
                    "name": host_display_name(),
                    "os_profile": detect_host_os_profile(),
                    "requires_token": is_connection_code_enabled(&state),
                    "port": DEFAULT_PORT,
                    "udp_port": UDP_MOVE_PORT,
                    "pointer_speed": state.pointer_speed.load(Ordering::Relaxed),
                    "power_actions_enabled": state.power_actions_enabled.load(Ordering::Relaxed) > 0
                })
                .to_string()
                .into(),
            ))
            .await;
        return Ok(());
    }

    let auth: AuthMessage =
        serde_json::from_value(first_value).map_err(|_| "invalid auth payload".to_string())?;

    if auth.kind != "auth" {
        return Err("first message must be auth".into());
    }

    if !is_token_authorized(&state, &auth.token) {
        let _ = ws
            .send(Message::Text("{\"type\":\"error\",\"reason\":\"unauthorized\"}".into()))
            .await;
        return Err("unauthorized token".into());
    }

    let device_id = register_device_connection(&state, &auth, &peer_ip);
    state.connections.fetch_add(1, Ordering::Relaxed);
    emit_connections(&app, &state);

    let _ = ws
        .send(Message::Text(
            json!({
                "type": "ready",
                "app": "wirelesspc",
                "id": host_device_id(),
                "os_profile": detect_host_os_profile(),
                "requires_token": is_connection_code_enabled(&state),
                "udp_port": UDP_MOVE_PORT,
                "pointer_speed": state.pointer_speed.load(Ordering::Relaxed),
                "power_actions_enabled": state.power_actions_enabled.load(Ordering::Relaxed) > 0
            })
            .to_string()
            .into(),
        ))
        .await;

    let ready_open_apps_payload = state
        .open_apps
        .lock()
        .ok()
        .map(|store| {
            json!({
                "type": "open_apps_list",
                "updated_at": store.updated_at,
                "items": open_apps_payload(&store)
            })
            .to_string()
        });
    if let Some(payload) = ready_open_apps_payload {
        let _ = ws.send(Message::Text(payload.into())).await;
    }

    let mut stats_tick = time::interval(Duration::from_millis(1500));
    stats_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut system = System::new_all();
    let mut last_bytes_in = state.bytes_in.load(Ordering::Relaxed);
    let mut estimated_media_state = "unknown".to_string();

    loop {
        tokio::select! {
            incoming = ws.next() => {
                let Some(incoming) = incoming else { break };
                let msg = match incoming {
                    Ok(m) => m,
                    Err(e) => {
                        unregister_device_connection(&state, &device_id);
                        state.connections.fetch_sub(1, Ordering::Relaxed);
                        emit_connections(&app, &state);
                        return Err(format!("receive error: {e}"));
                    }
                };

                if let Message::Text(text) = msg {
                    state
                        .bytes_in
                        .fetch_add(text.len(), Ordering::Relaxed);
                    let parsed: Result<ControlEvent, _> = serde_json::from_str(&text);
                    match parsed {
                Ok(ControlEvent::Move { dx, dy }) => {
                    let _ = WS_MOVE_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let _ = state.input_tx.send(InputCommand::Move { dx, dy });
                }
                Ok(ControlEvent::Ping { ts }) => {
                    let _ = ws
                        .send(Message::Text(
                            json!({
                                "type": "pong",
                                "ts": ts
                            })
                            .to_string()
                            .into(),
                        ))
                        .await;
                }
                Ok(ControlEvent::PointerSpeed { value }) => {
                    let clamped = value.clamp(MIN_POINTER_SPEED as i32, MAX_POINTER_SPEED as i32) as usize;
                    state.pointer_speed.store(clamped, Ordering::Relaxed);
                }
                Ok(ControlEvent::Click {
                    button,
                    state: click_state,
                }) => {
                    let _ = state
                        .input_tx
                        .send(InputCommand::Click {
                            button: button.clone(),
                            state: click_state.clone(),
                        });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!(
                                "click {button}{}",
                                click_state
                                    .as_deref()
                                    .map(|s| format!(" ({s})"))
                                    .unwrap_or_default()
                            ),
                        },
                    );
                }
                Ok(ControlEvent::Scroll { dy, dx }) => {
                    let dx_val = dx.unwrap_or(0.0);
                    let _ = state.input_tx.send(InputCommand::Scroll { dy, dx: dx_val });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("scroll dy={} dx={}", dy.round() as i32, dx_val.round() as i32),
                        },
                    );
                }
                Ok(ControlEvent::Key { key }) => {
                    let _ = state
                        .input_tx
                        .send(InputCommand::Key { key: key.clone() });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("key {key}"),
                        },
                    );
                }
                Ok(ControlEvent::Text { text }) => {
                    let _ = state
                        .input_tx
                        .send(InputCommand::Text { text: text.clone() });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("text len={}", text.len()),
                        },
                    );
                }
                Ok(ControlEvent::Shortcut { combo }) => {
                    let _ = state
                        .input_tx
                        .send(InputCommand::Shortcut { combo: combo.clone() });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("shortcut {combo}"),
                        },
                    );
                }
                Ok(ControlEvent::Media { action }) => {
                    let _ = state
                        .input_tx
                        .send(InputCommand::Media { action: action.clone() });
                    match action.trim().to_uppercase().as_str() {
                        "MEDIA_TOGGLE" | "PLAY_PAUSE" => {
                            estimated_media_state = if estimated_media_state == "playing" {
                                "paused".to_string()
                            } else {
                                "playing".to_string()
                            };
                        }
                        "MEDIA_NEXT" | "NEXT" | "MEDIA_PREV" | "PREVIOUS" => {
                            estimated_media_state = "playing".to_string();
                        }
                        "MEDIA_STOP" | "STOP" => {
                            estimated_media_state = "stopped".to_string();
                        }
                        _ => {}
                    }
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("media {action}"),
                        },
                    );
                    let _ = ws
                        .send(Message::Text(
                            json!({
                                "type": "media_status",
                                "state": estimated_media_state,
                                "source": "desktop_estimated",
                                "estimated": true
                            })
                            .to_string()
                            .into(),
                        ))
                        .await;
                }
                Ok(ControlEvent::System { action }) => {
                    if is_power_action(&action) && state.power_actions_enabled.load(Ordering::Relaxed) == 0 {
                        let _ = app.emit(
                            "ws-control-event",
                            EventPayload {
                                message: format!("system {action} blocked(power_actions_disabled)"),
                            },
                        );
                        continue;
                    }
                    let _ = state
                        .input_tx
                        .send(InputCommand::System { action: action.clone() });
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("system {action}"),
                        },
                    );
                }
                Ok(ControlEvent::ClipboardSet { text }) => {
                    let result = clipboard_set_text(&text);
                    let ok = result.is_ok();
                    let message = if ok {
                        "clipboard_set_ok".to_string()
                    } else {
                        result.err().unwrap_or_else(|| "clipboard_set_failed".to_string())
                    };
                    let _ = ws
                        .send(Message::Text(
                            json!({
                                "type": "clipboard_status",
                                "ok": ok,
                                "message": message
                            })
                            .to_string()
                            .into(),
                        ))
                        .await;
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: if ok { "clipboard set".to_string() } else { "clipboard set failed".to_string() },
                        },
                    );
                }
                Ok(ControlEvent::ClipboardGet) => {
                    let result = clipboard_get_text();
                    match result {
                        Ok(text) => {
                            let _ = ws
                                .send(Message::Text(
                                    json!({
                                        "type": "clipboard_value",
                                        "text": text
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                            let _ = app.emit(
                                "ws-control-event",
                                EventPayload {
                                    message: "clipboard get".to_string(),
                                },
                            );
                        }
                        Err(err) => {
                            let _ = ws
                                .send(Message::Text(
                                    json!({
                                        "type": "clipboard_status",
                                        "ok": false,
                                        "message": err
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await;
                        }
                    }
                }
                Ok(ControlEvent::OpenAppsGet) => {
                    let list_payload = state
                        .open_apps
                        .lock()
                        .ok()
                        .map(|store| {
                            json!({
                                "type": "open_apps_list",
                                "updated_at": store.updated_at,
                                "items": open_apps_payload(&store)
                            })
                            .to_string()
                        });
                    if let Some(payload) = list_payload {
                        let _ = ws.send(Message::Text(payload.into())).await;
                    }
                }
                Ok(ControlEvent::OpenAppOpen { id }) => {
                    let app_path = state
                        .open_apps
                        .lock()
                        .ok()
                        .and_then(|s| s.items.iter().find(|a| a.id == id).map(|a| a.path.clone()));
                    let ok = app_path.as_deref().map(launch_open_app).unwrap_or(false);
                    let _ = ws
                        .send(Message::Text(
                            json!({
                                "type": "open_apps_status",
                                "ok": ok,
                                "message": if ok { "app_opened" } else { "app_open_failed" }
                            })
                            .to_string()
                            .into(),
                        ))
                        .await;
                }
                Ok(ControlEvent::OpenAppAdd { path }) => {
                    let list_payload = if let Ok(entry) = build_open_app_entry(&path) {
                        if let Ok(mut store) = state.open_apps.lock() {
                            if store.items.len() >= OPEN_APPS_MAX {
                                None
                            } else {
                            let exists = store
                                .items
                                .iter()
                                .any(|i| i.path.eq_ignore_ascii_case(&entry.path));
                            if !exists {
                                store.items.push(entry);
                                store.updated_at = now_unix_ms();
                                persist_open_apps(&store);
                            }
                            Some(
                                json!({
                                    "type": "open_apps_list",
                                    "updated_at": store.updated_at,
                                    "items": open_apps_payload(&store)
                                })
                                .to_string(),
                            )
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(payload) = list_payload {
                        let _ = ws.send(Message::Text(payload.into())).await;
                    } else {
                        let _ = ws
                            .send(Message::Text(
                                json!({
                                    "type": "open_apps_status",
                                    "ok": false,
                                    "message": "app_add_failed_or_limit_reached"
                                })
                                .to_string()
                                .into(),
                            ))
                            .await;
                    }
                }
                Ok(ControlEvent::OpenAppRemove { id }) => {
                    let list_payload = if let Ok(mut store) = state.open_apps.lock() {
                        let before = store.items.len();
                        store.items.retain(|i| i.id != id);
                        if before != store.items.len() {
                            let _ = fs::remove_file(open_apps_icon_path(&id));
                            store.updated_at = now_unix_ms();
                            persist_open_apps(&store);
                        }
                        Some(
                            json!({
                                "type": "open_apps_list",
                                "updated_at": store.updated_at,
                                "items": open_apps_payload(&store)
                            })
                            .to_string(),
                        )
                    } else {
                        None
                    };
                    if let Some(payload) = list_payload {
                        let _ = ws.send(Message::Text(payload.into())).await;
                    }
                }
                Ok(ControlEvent::OpenAppsSync { items, updated_at }) => {
                    let list_payload = if let Ok(mut store) = state.open_apps.lock() {
                        if updated_at > store.updated_at {
                            let mut next_items = Vec::new();
                            for incoming in items {
                                if next_items.len() >= OPEN_APPS_MAX {
                                    break;
                                }
                                let built = build_open_app_entry(&incoming.path);
                                if let Ok(mut entry) = built {
                                    if let Some(id) = incoming.id.clone() {
                                        entry.id = id;
                                    }
                                    if !incoming.name.trim().is_empty() {
                                        entry.name = incoming.name;
                                    }
                                    if next_items.iter().any(|i: &OpenAppEntry| i.path.eq_ignore_ascii_case(&entry.path)) {
                                        continue;
                                    }
                                    next_items.push(entry);
                                }
                            }
                            store.items = next_items;
                            store.updated_at = updated_at;
                            persist_open_apps(&store);
                        }
                        Some(
                            json!({
                                "type": "open_apps_list",
                                "updated_at": store.updated_at,
                                "items": open_apps_payload(&store)
                            })
                            .to_string(),
                        )
                    } else {
                        None
                    };
                    if let Some(payload) = list_payload {
                        let _ = ws.send(Message::Text(payload.into())).await;
                    }
                }
                Err(_) => {
                    let _ = app.emit(
                        "ws-control-event",
                        EventPayload {
                            message: format!("event_error: invalid JSON payload={text}"),
                        },
                    );
                }
            }
                }
            }
            _ = stats_tick.tick() => {
                system.refresh_cpu_usage();
                system.refresh_memory();
                let cpu = system.global_cpu_usage();
                let mem_total_mb = (system.total_memory() / (1024 * 1024)) as u64;
                let mem_used_mb = (system.used_memory() / (1024 * 1024)) as u64;

                let current_bytes_in = state.bytes_in.load(Ordering::Relaxed);
                let delta = current_bytes_in.saturating_sub(last_bytes_in);
                last_bytes_in = current_bytes_in;
                let net_in_kbps = (delta as f64 / 1024.0) / 1.5;
                let uptime_sec = state.started_at.elapsed().as_secs();

                let payload = json!({
                    "type": "system_stats",
                    "host_name": host_display_name(),
                    "os_profile": detect_host_os_profile(),
                    "connections": state.connections.load(Ordering::Relaxed),
                    "pointer_speed": state.pointer_speed.load(Ordering::Relaxed),
                    "power_actions_enabled": state.power_actions_enabled.load(Ordering::Relaxed) > 0,
                    "cpu_percent": cpu,
                    "memory_used_mb": mem_used_mb,
                    "memory_total_mb": mem_total_mb,
                    "net_in_kbps": net_in_kbps,
                    "uptime_sec": uptime_sec
                });
                let _ = ws.send(Message::Text(payload.to_string().into())).await;
            }
        }
    }

    unregister_device_connection(&state, &device_id);
    state.connections.fetch_sub(1, Ordering::Relaxed);
    emit_connections(&app, &state);
    Ok(())
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum UdpPacket {
    #[serde(rename = "move")]
    Move { token: String, dx: f64, dy: f64 },
    #[serde(rename = "control")]
    Control {
        token: String,
        domain: String,
        action: String,
    },
}

async fn run_udp_server(state: AppState, app: AppHandle) {
    let socket = match UdpSocket::bind(("0.0.0.0", UDP_MOVE_PORT)).await {
        Ok(s) => s,
        Err(e) => {
            let _ = app.emit(
                "ws-control-event",
                EventPayload {
                    message: format!("udp_bind_error:{e}"),
                },
            );
            return;
        }
    };

    let mut buf = [0_u8; 2048];
    loop {
        let (len, _) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        if len == 0 {
            continue;
        }
        state.bytes_in.fetch_add(len, Ordering::Relaxed);

        let raw = match std::str::from_utf8(&buf[..len]) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let packet: UdpPacket = match serde_json::from_str(raw) {
            Ok(p) => p,
            Err(_) => continue,
        };

        match packet {
            UdpPacket::Move { token, dx, dy } => {
                if !is_token_authorized(&state, &token) {
                    continue;
                }
                let _ = UDP_MOVE_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
                let _ = state.input_tx.send(InputCommand::Move { dx, dy });
            }
            UdpPacket::Control {
                token,
                domain,
                action,
            } => {
                if !is_token_authorized(&state, &token) {
                    continue;
                }
                match domain.as_str() {
                    "media" => {
                        let _ = state.input_tx.send(InputCommand::Media { action });
                    }
                    "system" => {
                        if is_power_action(&action) && state.power_actions_enabled.load(Ordering::Relaxed) == 0 {
                            continue;
                        }
                        let _ = state.input_tx.send(InputCommand::System { action });
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn run_ws_server(state: AppState, app: AppHandle) {
    let listener = match TcpListener::bind(("0.0.0.0", DEFAULT_PORT)).await {
        Ok(l) => l,
        Err(e) => {
            let _ = app.emit(
                "ws-control-event",
                EventPayload {
                    message: format!("server_bind_error:{e}"),
                },
            );
            return;
        }
    };

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };

        let state_clone = state.clone();
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = handle_socket(stream, state_clone, app_clone).await;
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (input_tx, input_rx) = mpsc::unbounded_channel::<InputCommand>();

    let state = AppState {
        token: Arc::new(Mutex::new(generate_quick_code())),
        connections: Arc::new(AtomicUsize::new(0)),
        pointer_speed: Arc::new(AtomicUsize::new(DEFAULT_POINTER_SPEED)),
        power_actions_enabled: Arc::new(AtomicUsize::new(0)),
        connection_code_enabled: Arc::new(AtomicUsize::new(0)),
        connected_devices: Arc::new(Mutex::new(HashMap::new())),
        last_connected_device: Arc::new(Mutex::new(String::new())),
        started_at: Arc::new(Instant::now()),
        bytes_in: Arc::new(AtomicUsize::new(0)),
        input_tx,
        open_apps: Arc::new(Mutex::new(load_open_apps_store())),
    };

    tauri::Builder::default()
        .manage(state.clone())
        .setup(move |app| {
            let pref_item = MenuItemBuilder::with_id("open_preferences", "Preferences")
                .build(app)?;
            let qr_item = MenuItemBuilder::with_id("open_qr", "QR Code")
                .build(app)?;
            let apps_item = MenuItemBuilder::with_id("open_apps", "Apps")
                .build(app)?;
            let qa_item = MenuItemBuilder::with_id("open_qa", "QA")
                .build(app)?;
            let updates_item = MenuItemBuilder::with_id("check_updates", "Check for Updates")
                .build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&pref_item)
                .item(&qr_item)
                .item(&apps_item)
                .item(&qa_item)
                .item(&updates_item)
                .separator()
                .item(&quit_item)
                .build()?;

            let mut tray_builder = TrayIconBuilder::new()
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app_handle, event: MenuEvent| match event.id().as_ref() {
                    "open_preferences" => {
                        show_main_window(app_handle);
                        let _ = app_handle.emit("open-preferences", ());
                    }
                    "open_qr" => {
                        show_main_window(app_handle);
                        let _ = app_handle.emit("open-qr", ());
                    }
                    "open_apps" => {
                        show_main_window(app_handle);
                        let _ = app_handle.emit("open-apps", ());
                    }
                    "check_updates" => {
                        show_main_window(app_handle);
                        let _ = app_handle.emit("check-updates", ());
                    }
                    "open_qa" => {
                        let _ = app_handle
                            .opener()
                            .open_url("https://wirelesspc.arolisg.dev/qa", None::<&str>);
                    }
                    "quit" => app_handle.exit(0),
                    _ => {}
                });

            tray_builder = tray_builder.icon(tauri::include_image!("icons/icon.png"));

            tray_builder.build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_main_window(&app_handle);
                    }
                });
            }

            let app_handle = app.handle().clone();
            let managed_state = app.state::<AppState>().inner().clone();
            let token_now = current_token(&managed_state);
            let requires_token = is_connection_code_enabled(&managed_state);
            if let Ok(mdns) = announce_mdns(&token_now, requires_token) {
                // Keep daemon alive for app lifetime.
                std::mem::forget(mdns);
            }

            tauri::async_runtime::spawn(async move {
                run_ws_server(managed_state, app_handle).await;
            });
            let udp_state = app.state::<AppState>().inner().clone();
            let udp_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                run_udp_server(udp_state, udp_app).await;
            });
            let pointer_speed = app.state::<AppState>().pointer_speed.clone();
            tauri::async_runtime::spawn(async move {
                run_input_worker(input_rx, pointer_speed).await;
            });

            Ok(())
        })
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            generate_pairing_info,
            set_power_actions_enabled,
            get_power_actions_enabled,
            set_connection_code_enabled,
            get_connection_code_enabled,
            get_launch_on_startup,
            set_launch_on_startup,
            clear_connected_devices_history,
            get_open_apps,
            add_open_app,
            remove_open_app,
            discover_open_apps,
            pick_open_app_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



