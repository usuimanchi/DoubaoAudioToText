//! 豆包语音转文字 — Tauri 库入口

mod commands;

/// 应用全局状态
pub struct AppState {
    pub active_jobs: std::sync::Mutex<std::collections::HashMap<String, ()>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            active_jobs: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_transcription,
            commands::open_path,
            commands::resolve_ffmpeg_path,
            commands::load_last_api_key,
            commands::save_last_api_key,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
