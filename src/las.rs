//! 火山引擎 LAS 算子（las_asr_pro）转录后端
//!
//! 新版本 API，基于 LAS（数据处理智能平台）算子：
//! - 不限文件大小和时长，无需切分
//! - 支持视频文件（mp4/mov/mkv）
//! - 支持 99 种语言
//! - 内置音频降噪
//! - Bearer Token 鉴权

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

use crate::backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
use crate::types::{Config, PreparedChunk, SubmittedTaskSummary, Provider};

// ---------------------------------------------------------------------------
// LAS 请求类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct LasSubmitRequest {
    operator_id: String,
    operator_version: String,
    data: LasSubmitData,
}

#[derive(Debug, Clone, Serialize)]
struct LasSubmitData {
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<LasUser>,
    audio: LasAudio,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
    request: LasRequest,
}

#[derive(Debug, Clone, Serialize)]
struct LasUser {
    uid: String,
}

#[derive(Debug, Clone, Serialize)]
struct LasAudio {
    url: String,
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bits: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LasRequest {
    model_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_itn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_punc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_ddc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_speaker_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_channel_split: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_utterances: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_speech_rate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_volume: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_lid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_emotion_detection: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_gender_detection: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_auto_lang: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_denoise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_multi_language: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_poi_fc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_music_fc: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vad_segment: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_window_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sensitive_words_filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<LasCorpus>,
}

#[derive(Debug, Clone, Serialize)]
struct LasCorpus {
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boosting_table_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correct_table_name: Option<String>,
}

// ---------------------------------------------------------------------------
// LAS Poll 请求
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct LasPollRequest {
    operator_id: String,
    operator_version: String,
    task_id: String,
}

// ---------------------------------------------------------------------------
// LAS 响应类型
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LasSubmitResponse {
    metadata: LasMetadata,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LasPollResponse {
    metadata: LasMetadata,
    #[serde(default)]
    data: Option<LasPollData>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LasMetadata {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    task_status: Option<String>,
    #[serde(default)]
    business_code: Option<String>,
    #[serde(default)]
    error_msg: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LasPollData {
    #[serde(default)]
    audio_info: Option<Value>,
    #[serde(default)]
    result: Option<LasResult>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LasResult {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    utterances: Option<Vec<Value>>,
    #[serde(default)]
    additions: Option<Value>,
}

// ---------------------------------------------------------------------------
// LAS Backend
// ---------------------------------------------------------------------------

pub struct LasBackend;

fn las_submit_url(region: &str) -> String {
    format!("https://operator.las.{region}.volces.com/api/v1/submit")
}

fn las_poll_url(region: &str) -> String {
    format!("https://operator.las.{region}.volces.com/api/v1/poll")
}

#[async_trait]
impl TranscriptionBackend for LasBackend {
    fn provider_name() -> &'static str {
        "火山引擎 LAS 算子（las_asr_pro）"
    }

    async fn submit(
        client: &Client,
        config: &Config,
        chunk: &PreparedChunk,
    ) -> Result<JobHandle> {
        let file_url = chunk
            .submission_url
            .as_ref()
            .ok_or_else(|| anyhow!("LAS: 片段 {} 没有提交 URL", chunk.path.display()))?;

        let region = &config.las_region;

        // ---- 构建 request ----
        let las_request = LasRequest {
            model_name: "bigmodel".to_string(),
            model_version: config.model_version.clone(),
            language: config.language.clone(),
            enable_itn: bool_or_none(config.enable_itn, true),
            enable_punc: bool_or_none(config.enable_punc, true),
            enable_ddc: bool_or_none(config.enable_ddc, false),
            enable_speaker_info: bool_or_none(config.enable_speaker_info, false),
            enable_channel_split: bool_or_none(config.enable_channel_split, false),
            show_utterances: bool_or_none(config.show_utterances, false),
            show_speech_rate: bool_or_none(config.show_speech_rate, false),
            show_volume: bool_or_none(config.show_volume, false),
            enable_lid: bool_or_none(config.enable_lid, false),
            enable_emotion_detection: bool_or_none(config.enable_emotion_detection, false),
            enable_gender_detection: bool_or_none(config.enable_gender_detection, false),
            enable_auto_lang: bool_or_none(config.enable_auto_lang, true),
            enable_denoise: bool_or_none(config.enable_denoise, false),
            enable_multi_language: bool_or_none(config.enable_multi_language, true),
            enable_poi_fc: bool_or_none(config.enable_poi_fc, false),
            enable_music_fc: bool_or_none(config.enable_music_fc, false),
            vad_segment: if config.enable_speaker_info { Some(true) } else { None },
            end_window_size: config.end_window_size,
            sensitive_words_filter: config.sensitive_words_filter.clone(),
            corpus: build_las_corpus(config),
        };

        let body = LasSubmitRequest {
            operator_id: "las_asr_pro".to_string(),
            operator_version: config.operator_version.clone(),
            data: LasSubmitData {
                user: Some(LasUser {
                    uid: "rust-client".to_string(),
                }),
                audio: LasAudio {
                    url: file_url.to_string(),
                    format: chunk.format.clone(),
                    codec: if chunk.codec == "raw" && chunk.format == "raw" {
                        None
                    } else {
                        Some(chunk.codec.clone())
                    },
                    rate: Some(chunk.sample_rate),
                    bits: Some(16),
                    channel: Some(1),
                    language: config.language.clone(),
                },
                resource: Some(config.resource_id.clone()),
                request: las_request,
            },
        };

        let resp = client
            .post(&las_submit_url(region))
            .header(
                "Authorization",
                format!("Bearer {}", &config.api_key),
            )
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("LAS 提交任务失败")?;

        if !resp.status().is_success() {
            let http_status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "LAS 提交失败 HTTP {}: {}",
                http_status,
                &err_body[..err_body.len().min(500)]
            ));
        }

        let submit_resp: LasSubmitResponse = resp
            .json()
            .await
            .context("解析 LAS 提交响应失败")?;

        let task_id = submit_resp
            .metadata
            .task_id
            .ok_or_else(|| {
                anyhow!(
                    "LAS 提交响应中无 task_id: error_msg={:?}",
                    submit_resp.metadata.error_msg
                )
            })?;

        println!("   ✅ LAS 任务已提交  task_id={}", &task_id[..16.min(task_id.len())]);

        Ok(JobHandle {
            id: task_id,
            query_url: None,
            provider: Provider::Las,
        })
    }

    async fn wait_for_completion(
        client: &Client,
        config: &Config,
        handle: &JobHandle,
    ) -> Result<TranscriptionOutput> {
        let region = &config.las_region;
        let poll_interval = Duration::from_secs(config.poll_interval_secs);
        let start = std::time::Instant::now();
        let mut tries = 0u32;

        let poll_body = LasPollRequest {
            operator_id: "las_asr_pro".to_string(),
            operator_version: config.operator_version.clone(),
            task_id: handle.id.clone(),
        };

        loop {
            tries += 1;

            let resp = client
                .post(&las_poll_url(region))
                .header(
                    "Authorization",
                    format!("Bearer {}", &config.api_key),
                )
                .header(CONTENT_TYPE, "application/json")
                .json(&poll_body)
                .send()
                .await
                .context("LAS 查询任务状态失败")?;

            if !resp.status().is_success() {
                let http_status = resp.status();
                let err_body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "LAS 查询失败 HTTP {}: {}",
                    http_status,
                    &err_body[..err_body.len().min(500)]
                ));
            }

            let poll_resp: LasPollResponse = resp
                .json()
                .await
                .context("解析 LAS 查询响应失败")?;

            let task_status = poll_resp.metadata.task_status.as_deref().unwrap_or("UNKNOWN");

            match task_status {
                "COMPLETED" => {
                    let elapsed = start.elapsed();

                    // 提取文本
                    let text = poll_resp
                        .data
                        .as_ref()
                        .and_then(|d| d.result.as_ref())
                        .and_then(|r| r.text.clone());

                    let text_preview = text.as_deref().unwrap_or("（无文本）");
                    let preview = if text_preview.len() > 60 {
                        format!("{}...", &text_preview[..60])
                    } else {
                        text_preview.to_string()
                    };

                    println!(
                        "   ✅ LAS 任务完成  task_id={}  耗时={:.0}s  结果: {}",
                        &handle.id[..16.min(handle.id.len())],
                        elapsed.as_secs(),
                        preview
                    );

                    return Ok(TranscriptionOutput {
                        raw_json: serde_json::to_value(&poll_resp).unwrap_or(Value::Null),
                        text,
                    });
                }
                "PENDING" | "RUNNING" | "PROCESSING" => {
                    if tries % 12 == 1 {
                        let elapsed = start.elapsed();
                        println!(
                            "   ⏳ 等待中  task_id={}  已等待 {:.0}s  status={}",
                            &handle.id[..16.min(handle.id.len())],
                            elapsed.as_secs(),
                            task_status
                        );
                    }
                    sleep(poll_interval).await;
                }
                "FAILED" | "TIMEOUT" => {
                    let error_msg = poll_resp
                        .metadata
                        .error_msg
                        .unwrap_or_else(|| "未知错误".to_string());
                    return Err(anyhow!("LAS 任务失败 [{}]: {}", task_status, error_msg));
                }
                _ => {
                    if tries > 360 {
                        // 30 分钟超时
                        return Err(anyhow!("LAS 轮询超时: 未知状态 '{}'", task_status));
                    }
                    println!(
                        "   ⚠️  未知状态，继续轮询  task_id={}  status={}",
                        &handle.id[..16.min(handle.id.len())],
                        task_status
                    );
                    sleep(poll_interval).await;
                }
            }
        }
    }

    fn save_result(
        config: &Config,
        handle: &JobHandle,
        output: &TranscriptionOutput,
        chunk: &PreparedChunk,
    ) -> Result<SubmittedTaskSummary> {
        let result_dir = config.output_dir.join("results").join(&handle.id);
        fs::create_dir_all(&result_dir)?;

        let json_path = result_dir.join("response.json");
        fs::write(&json_path, serde_json::to_vec_pretty(&output.raw_json)?)?;

        if let Some(ref text) = output.text {
            let txt_path = result_dir.join("result.txt");
            fs::write(&txt_path, text)?;
            println!("   📝 文本已保存: {}", txt_path.display());
        }

        // 提取 utterances 为 SRT 字幕文件
        if let Some(utterances) = output
            .raw_json
            .get("data")
            .and_then(|d| d.get("result"))
            .and_then(|r| r.get("utterances"))
            .and_then(|u| u.as_array())
        {
            if !utterances.is_empty() {
                let srt = build_srt(utterances);
                let srt_path = result_dir.join("result.srt");
                fs::write(&srt_path, &srt)?;
                println!("   📝 字幕已保存: {}", srt_path.display());
            }
        }

        Ok(SubmittedTaskSummary {
            request_id: handle.id.clone(),
            chunk_path: chunk.path.display().to_string(),
            submission_url: chunk.submission_url.clone().unwrap_or_default(),
            status_code: Some(0),
            status_message: Some("COMPLETED".to_string()),
            result_text: output.text.clone(),
            result_json_path: Some(json_path.display().to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 只在值不为默认值时才输出（减少请求体中的噪音）
fn bool_or_none(val: bool, default: bool) -> Option<bool> {
    if val == default {
        None
    } else {
        Some(val)
    }
}

fn build_las_corpus(config: &Config) -> Option<LasCorpus> {
    let has_context = config
        .corpus_context
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_boosting = config
        .boosting_table_name
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_correct = config
        .correct_table_name
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    if !has_context && !has_boosting && !has_correct {
        return None;
    }

    Some(LasCorpus {
        context: config.corpus_context.clone(),
        boosting_table_name: config.boosting_table_name.clone(),
        correct_table_name: config.correct_table_name.clone(),
    })
}

/// 从 utterances 构建 SRT 字幕
fn build_srt(utterances: &[Value]) -> String {
    let mut srt = String::new();
    for (i, u) in utterances.iter().enumerate() {
        let text = u
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let start_ms = u
            .get("start_time")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let end_ms = u
            .get("end_time")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        srt.push_str(&format!("{}\n", i + 1));
        srt.push_str(&format!(
            "{} --> {}\n",
            ms_to_srt_time(start_ms),
            ms_to_srt_time(end_ms)
        ));
        srt.push_str(&format!("{}\n\n", text));
    }
    srt
}

fn ms_to_srt_time(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let ms_part = ms % 1000;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms_part)
}
