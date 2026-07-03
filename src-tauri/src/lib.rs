//! 豆包语音转文字 — Tauri 库入口

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use volc_core::{
    progress::{ProgressEvent, ProgressReporter},
    types::*,
};

pub struct AppState {
    pub active_jobs: Mutex<std::collections::HashMap<String, ()>>,
}

// ---------------------------------------------------------------------------
// 数据目录辅助
// ---------------------------------------------------------------------------

fn app_data_dir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn ensure_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建数据目录: {e}"))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// TauriProgressReporter
// ---------------------------------------------------------------------------

struct TauriProgressReporter {
    app: AppHandle,
    job_id: String,
}

impl ProgressReporter for TauriProgressReporter {
    fn emit(&self, event: ProgressEvent) {
        let _ = self.app.emit(
            "progress",
            json!({ "job_id": self.job_id, "event": event }),
        );
    }
    fn log(&self, msg: String) {
        self.emit(ProgressEvent::Log { level: volc_core::progress::LogLevel::Info, msg });
    }
    fn warn(&self, msg: String) {
        self.emit(ProgressEvent::Log { level: volc_core::progress::LogLevel::Warn, msg });
    }
    fn error(&self, msg: String) {
        self.emit(ProgressEvent::Log { level: volc_core::progress::LogLevel::Error, msg });
    }
}

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct TranscriptionConfigInput {
    provider: String,
    api_key: Option<String>,
    azure_key: Option<String>,
    azure_region: Option<String>,
    language: Option<String>,
    output_dir: Option<String>,
    prepare_only: Option<bool>,
    ark_model: Option<String>,
}

fn build_config_from_input(
    app: &AppHandle,
    input: &TranscriptionConfigInput,
    reporter: Arc<TauriProgressReporter>,
    extra_bin_dirs: Vec<PathBuf>,
) -> Result<volc_core::Config, String> {
    let provider = match input.provider.as_str() {
        "azure" | "Azure" => Provider::Azure,
        "las" | "LAS" => Provider::Las,
        "ark" | "Ark" | "ARK" => Provider::Ark,
        "volcengine" | "volc" => Provider::Volcengine,
        _ => Provider::Ark,
    };
    let api_key = input.api_key.clone().unwrap_or_default();

    // 输出目录：用户指定 > 桌面 > 文档 > 当前目录
    let default_out_dir = if let Ok(desktop) = dirs_next::desktop_dir() {
        desktop.join("音频转写结果")
    } else if let Ok(docs) = dirs_next::document_dir() {
        docs.join("音频转写结果")
    } else {
        PathBuf::from("./结果")
    };

    let output_dir = input
        .output_dir
        .clone()
        .filter(|d| !d.trim().is_empty() && d != "./result")
        .map(PathBuf::from)
        .unwrap_or(default_out_dir);

    app_data_dir(app); // ensure app data dir exists for later use

    let (max_dur, max_size) = match provider {
        Provider::Ark => (7170u64, 512 * 1024 * 1024),
        Provider::Las => (u64::MAX, u64::MAX),
        _ => (7170, 25 * 1024 * 1024),
    };
    Ok(volc_core::Config {
        provider,
        api_key,
        resource_id: "seedasr".into(),
        legacy_mode: false,
        app_key: None,
        access_key: None,
        azure_key: input.azure_key.clone(),
        azure_region: input.azure_region.clone(),
        language: input.language.clone(),
        enable_speaker_info: true,
        enable_itn: true,
        enable_punc: true,
        enable_ddc: true,
        enable_auto_lang: true,
        show_utterances: false,
        end_window_size: None,
        boosting_table_name: None,
        correct_table_name: None,
        corpus_context: None,
        ark_model: input.ark_model.clone().unwrap_or_else(|| "doubao-seed-2-0-lite-260428".into()),
        las_region: "cn-beijing".into(),
        operator_version: "v2".into(),
        model_version: None,
        enable_denoise: true,
        enable_multi_language: true,
        enable_channel_split: false,
        show_speech_rate: false,
        show_volume: false,
        enable_lid: false,
        enable_emotion_detection: false,
        enable_gender_detection: false,
        sensitive_words_filter: None,
        enable_poi_fc: false,
        enable_music_fc: false,
        candidate_locales: None,
        word_level_timestamps: false,
        profanity_filter_mode: "Masked".into(),
        punctuation_mode: "DictatedAndAutomatic".into(),
        output_dir,
        poll_interval_secs: 5,
        max_duration_secs: max_dur,
        max_size_bytes: max_size,
        prepare_only: input.prepare_only.unwrap_or(false),
        max_split_depth: 3,
        target_audio_format: if provider == Provider::Ark { "mp3".into() } else { "ogg".into() },
        reporter: reporter as Arc<dyn ProgressReporter + Send + Sync>,
        extra_bin_dirs,
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn pick_audio_files(app: AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        let files = app2
            .dialog()
            .file()
            .add_filter("音频", &["mp3", "wav", "ogg", "m4a", "aac", "flac", "mp4", "webm", "opus"])
            .blocking_pick_files();
        match files {
            Some(paths) => Ok(paths.into_iter().filter_map(|p| Some(p.as_path()?.to_string_lossy().to_string())).collect()),
            None => Ok(vec![]),
        }
    })
    .await
    .map_err(|e| format!("文件选择失败: {e}"))?
}

#[tauri::command]
async fn pick_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        let dir = app2.dialog().file().blocking_pick_folder();
        Ok(dir.and_then(|d| d.as_path().map(|p| p.to_string_lossy().to_string())))
    })
    .await
    .map_err(|e| format!("目录选择失败: {e}"))?
}

#[tauri::command]
async fn start_transcription(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    config_input: TranscriptionConfigInput,
    inputs: Vec<String>,
) -> Result<String, String> {
    // 校验
    if inputs.is_empty() {
        return Err("请添加至少一个音频文件".into());
    }
    let api_key = config_input.api_key.as_deref().unwrap_or("").trim().to_string();
    if api_key.is_empty() {
        return Err("请输入 API Key".into());
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    state.active_jobs.lock().unwrap().insert(job_id.clone(), ());

    let reporter = Arc::new(TauriProgressReporter {
        app: app.clone(),
        job_id: job_id.clone(),
    });

    let extra_bin_dirs: Vec<PathBuf> = match app.path().resource_dir() {
        Ok(d) => {
            let data = app_data_dir(&app);
            vec![d.join("binaries"), d, data]
        }
        Err(_) => vec![app_data_dir(&app)],
    };

    let mut config = build_config_from_input(&app, &config_input, reporter, extra_bin_dirs)?;
    let app2 = app.clone();
    let job_id2 = job_id.clone();

    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = app2.emit("job_done", json!({
                    "job_id": job_id2,
                    "success": false,
                    "error": format!("创建网络客户端失败: {e}"),
                    "output_dir": config.output_dir.to_string_lossy(),
                }));
                app2.state::<AppState>().active_jobs.lock().unwrap().remove(&job_id2);
                return;
            }
        };

        let result = volc_core::pipeline::run_pipeline_for_provider(&client, &mut config, &inputs).await;
        let _ = app2.emit("job_done", json!({
            "job_id": job_id2,
            "success": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
            "output_dir": config.output_dir.to_string_lossy(),
        }));
        app2.state::<AppState>().active_jobs.lock().unwrap().remove(&job_id2);
    });

    Ok(job_id)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    // 转为绝对路径，避免目录切换时找不到
    let p = std::path::Path::new(&path);
    let abs = if p.is_absolute() {
        path
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(&path).to_string_lossy().to_string(),
            Err(_) => path,
        }
    };
    opener::open(&abs).map_err(|e| format!("打开失败: {e}"))
}

#[tauri::command]
fn save_last_api_key(app: AppHandle, key: String) -> Result<(), String> {
    let dir = ensure_data_dir(&app)?;
    let path = dir.join(".last_api_key");
    volc_core::output::persist_api_key_hint(&path, &key).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_last_api_key(app: AppHandle) -> Result<String, String> {
    let dir = ensure_data_dir(&app)?;
    let path = dir.join(".last_api_key");
    volc_core::output::load_last_api_key_hint(&path).ok_or_else(|| "无已保存的 API Key".into())
}

#[tauri::command]
fn get_default_output_dir(app: AppHandle) -> Result<String, String> {
    // Just to show the default path in frontend
    Ok(app_data_dir(&app).to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            active_jobs: Mutex::new(std::collections::HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            pick_audio_files,
            pick_directory,
            start_transcription,
            open_path,
            save_last_api_key,
            load_last_api_key,
            get_default_output_dir,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
