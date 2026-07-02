//! Tauri 后端命令：启动转录、打开文件、API Key 管理
//!
//! 通过 `TauriProgressReporter` 将进度事件 emit 给前端。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use volc_core::{
    self,
    progress::{ProgressEvent, ProgressReporter},
    types::*,
};

use crate::AppState;

// ---------------------------------------------------------------------------
// TauriProgressReporter
// ---------------------------------------------------------------------------

/// 把进度事件通过 Tauri IPC emit 到前端 `progress` 事件
pub struct TauriProgressReporter {
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
        self.emit(ProgressEvent::Log {
            level: volc_core::progress::LogLevel::Info,
            msg,
        });
    }

    fn warn(&self, msg: String) {
        self.emit(ProgressEvent::Log {
            level: volc_core::progress::LogLevel::Warn,
            msg,
        });
    }

    fn error(&self, msg: String) {
        self.emit(ProgressEvent::Log {
            level: volc_core::progress::LogLevel::Error,
            msg,
        });
    }
}

// ---------------------------------------------------------------------------
// Tauri 配置（从前端表单 JSON 接收）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct TranscriptionConfigInput {
    pub provider: String,
    pub api_key: Option<String>,
    pub azure_key: Option<String>,
    pub azure_region: Option<String>,
    pub language: Option<String>,
    pub output_dir: Option<String>,
    pub prepare_only: Option<bool>,
    pub ark_model: Option<String>,
}

fn build_config_from_input(
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
        ark_model: input
            .ark_model
            .clone()
            .unwrap_or_else(|| "doubao-seed-2-0-lite-260428".into()),
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
        output_dir: input
            .output_dir
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./result")),
        poll_interval_secs: 5,
        max_duration_secs: max_dur,
        max_size_bytes: max_size,
        prepare_only: input.prepare_only.unwrap_or(false),
        max_split_depth: 3,
        target_audio_format: if provider == Provider::Ark {
            "mp3".into()
        } else {
            "ogg".into()
        },
        reporter: reporter as Arc<dyn volc_core::ProgressReporter + Send + Sync>,
        extra_bin_dirs,
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn start_transcription(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    config_input: TranscriptionConfigInput,
    inputs: Vec<String>,
) -> Result<String, String> {
    let job_id = uuid::Uuid::new_v4().to_string();

    // 记录活跃任务
    state.active_jobs.lock().unwrap().insert(job_id.clone(), ());

    let reporter = Arc::new(TauriProgressReporter {
        app: app.clone(),
        job_id: job_id.clone(),
    });

    // ffmpeg 搜索目录：Tauri resource_dir
    let extra_bin_dirs: Vec<PathBuf> = match app.path().resource_dir() {
        Ok(d) => {
            let binaries = d.join("binaries");
            vec![binaries, d]
        }
        Err(_) => vec![],
    };

    let mut config = build_config_from_input(&config_input, reporter, extra_bin_dirs)?;

    let app2 = app.clone();
    let job_id2 = job_id.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .unwrap();

        let result =
            volc_core::pipeline::run_pipeline_for_provider(&client, &mut config, &inputs).await;

        let _ = app2.emit(
            "job_done",
            json!({
                "job_id": job_id2,
                "success": result.is_ok(),
                "error": result.as_ref().err().map(|e| e.to_string()),
                "output_dir": config.output_dir.to_string_lossy(),
            }),
        );

        // 清理
        app2.state::<AppState>()
            .active_jobs
            .lock()
            .unwrap()
            .remove(&job_id2);
    });

    Ok(job_id)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    opener::open(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn resolve_ffmpeg_path(app: AppHandle) -> Result<String, String> {
    let extra_dirs: Vec<PathBuf> = match app.path().resource_dir() {
        Ok(d) => vec![d.join("binaries"), d],
        Err(_) => vec![],
    };
    let resolved = volc_core::audio::resolve_ffmpeg(&extra_dirs);
    Ok(resolved.to_string_lossy().to_string())
}

#[tauri::command]
fn load_last_api_key() -> Result<String, String> {
    let path = PathBuf::from("./result/.last_api_key");
    volc_core::output::load_last_api_key_hint(&path).ok_or_else(|| "无已保存的 API Key".into())
}

#[tauri::command]
fn save_last_api_key(key: String) -> Result<(), String> {
    let path = PathBuf::from("./result/.last_api_key");
    volc_core::output::persist_api_key_hint(&path, &key).map_err(|e| e.to_string())
}
