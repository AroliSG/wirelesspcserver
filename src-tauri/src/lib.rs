use rand::distr::{Alphanumeric, SampleString};
use serde::Serialize;

const DEFAULT_PORT: u16 = 39393;

#[derive(Serialize)]
struct PairingInfo {
    host: String,
    port: u16,
    token: String,
    ws_url: String,
}

#[tauri::command]
fn generate_pairing_info() -> PairingInfo {
    let host = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let token = Alphanumeric.sample_string(&mut rand::rng(), 24);
    let ws_url = format!("ws://{}:{}?token={}", host, DEFAULT_PORT, token);

    PairingInfo {
        host,
        port: DEFAULT_PORT,
        token,
        ws_url,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![generate_pairing_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
