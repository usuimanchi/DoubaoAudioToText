//! 火山引擎（豆包大模型）转录后端

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
use crate::types::{Config, PreparedChunk, SubmittedTaskSummary, Provider};

const SUBMIT_URL: &str = "https://openspeech.bytedance.com/api/v3/auc/bigmodel/submit";
const QUERY_URL: &str = "https://openspeech.bytedance.com/api/v3/auc/bigmodel/query";

// ---------------------------------------------------------------------------
// Volcengine 请求/响应类型（私有）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct SubmitRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<User<'a>>,
    audio: AudioBlock<'a>,
    request: RequestBlock<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callback: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callback_data: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
struct User<'a> {
    uid: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct AudioBlock<'a> {
    url: &'a str,
    format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    codec: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bits: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
struct RequestBlock<'a> {
    model_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    enable_itn: bool,
    enable_punc: bool,
    enable_ddc: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_speaker_info: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_auto_lang: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_utterances: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vad_segment: Option<bool>,
    /// 强制判停时间（300-5000ms），设置后按静音时长分句而非语义分句
    #[serde(skip_serializing_if = "Option::is_none")]
    end_window_size: Option<u32>,
    /// 语料/干预词容器
    #[serde(skip_serializing_if = "Option::is_none")]
    corpus: Option<CorpusBlock>,
}

/// 语料/干预词配置
#[derive(Debug, Clone, Serialize)]
struct CorpusBlock {
    /// 自学习平台热词词表名称
    #[serde(skip_serializing_if = "Option::is_none")]
    boosting_table_name: Option<String>,
    /// 自学习平台替换词词表名称
    #[serde(skip_serializing_if = "Option::is_none")]
    correct_table_name: Option<String>,
    /// 上下文 JSON 字符串（hotwords / dialog_ctx / loc_info 等）
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct QueryResponse {
    #[serde(default)]
    audio_info: Option<AudioInfo>,
    #[serde(default)]
    result: Option<ResultBlock>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AudioInfo {
    #[serde(default)]
    duration: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ResultBlock {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    utterances: Option<Vec<Utterance>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Utterance {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    start_time: Option<u64>,
    #[serde(default)]
    end_time: Option<u64>,
}

#[derive(Debug, Clone)]
struct ApiStatus {
    code: Option<i64>,
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Volcengine Backend
// ---------------------------------------------------------------------------

pub struct VolcengineBackend;

#[async_trait]
impl TranscriptionBackend for VolcengineBackend {
    fn provider_name() -> &'static str {
        "火山引擎（豆包大模型）"
    }

    async fn submit(
        client: &Client,
        config: &Config,
        chunk: &PreparedChunk,
    ) -> Result<JobHandle> {
        let file_url = chunk.submission_url.as_ref()
            .ok_or_else(|| anyhow!("片段 {} 没有提交 URL", chunk.path.display()))?;

        let request_id = Uuid::new_v4().to_string();
        let headers = build_headers(config, &request_id)?;

        let audio_format = &chunk.format[..];
        let audio_codec: Option<&str> = if chunk.codec == "raw" && chunk.format == "raw" {
            None
        } else {
            Some(&chunk.codec[..])
        };

        let request = SubmitRequest {
            user: Some(User { uid: "rust-client" }),
            audio: AudioBlock {
                url: file_url,
                format: audio_format,
                codec: audio_codec,
                rate: Some(chunk.sample_rate),
                bits: Some(16),
                channel: Some(1),
                language: None,
            },
            request: RequestBlock {
                model_name: "bigmodel",
                language: config.language.as_deref(),
                enable_itn: config.enable_itn,
                enable_punc: config.enable_punc,
                enable_ddc: config.enable_ddc,
                enable_speaker_info: if config.enable_speaker_info { Some(true) } else { None },
                enable_auto_lang: if config.enable_auto_lang { Some(true) } else { None },
                show_utterances: if config.show_utterances { Some(true) } else { None },
                vad_segment: if config.enable_speaker_info { Some(true) } else { None },
                end_window_size: config.end_window_size,
                corpus: build_corpus(config),
            },
            callback: None,
            callback_data: None,
        };

        let resp = client
            .post(SUBMIT_URL)
            .headers(headers)
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await
            .context("提交任务网络请求失败")?;

        let status = parse_status_headers(resp.headers());
        match status.code {
            Some(20000000) => Ok(JobHandle {
                id: request_id,
                query_url: None,
                provider: Provider::Volcengine,
                operator_version: None,
            }),
            Some(code) => Err(anyhow!(
                "提交失败: 状态码={}, 消息={:?}",
                code,
                status.message.unwrap_or_else(|| "未知错误".to_string())
            )),
            None => {
                let body = resp.text().await.unwrap_or_default();
                Err(anyhow!("提交失败: 未返回状态码, body={}", &body[..body.len().min(500)]))
            }
        }
    }

    async fn wait_for_completion(
        client: &Client,
        config: &Config,
        handle: &JobHandle,
    ) -> Result<TranscriptionOutput> {
        let mut tries = 0u32;
        let start_time = std::time::Instant::now();

        loop {
            tries += 1;
            let headers = build_headers(config, &handle.id)?;

            let resp = client
                .post(QUERY_URL)
                .headers(headers)
                .header(CONTENT_TYPE, "application/json")
                .json(&serde_json::json!({}))
                .send()
                .await
                .context("查询任务网络请求失败")?;

            let status = parse_status_headers(resp.headers());
            let body_text = resp.text().await.unwrap_or_default();
            let parsed: QueryResponse = serde_json::from_str(&body_text).unwrap_or(QueryResponse {
                audio_info: None,
                result: None,
            });

            match status.code {
                Some(20000000) => {
                    let elapsed = start_time.elapsed();
                    let text_preview = parsed
                        .result
                        .as_ref()
                        .and_then(|r| r.text.as_deref())
                        .unwrap_or("（无文本）");
                    let preview = if text_preview.len() > 60 {
                        format!("{}...", &text_preview[..60])
                    } else {
                        text_preview.to_string()
                    };
                    println!(
                        "   ✅ 完成  request_id={}  耗时={:.0}s  结果: {}",
                        &handle.id[..8],
                        elapsed.as_secs(),
                        preview
                    );
                    let text = parsed.result.as_ref().and_then(|r| r.text.clone());
                    return Ok(TranscriptionOutput {
                        raw_json: serde_json::to_value(&parsed).unwrap_or(Value::Null),
                        text,
                    });
                }
                Some(20000001) | Some(20000002) => {
                    if tries % 12 == 1 {
                        let elapsed = start_time.elapsed();
                        println!(
                            "   ⏳ 等待中  request_id={}  已等待 {:.0}s  tries={}",
                            &handle.id[..8],
                            elapsed.as_secs(),
                            tries
                        );
                    }
                    sleep(Duration::from_secs(config.poll_interval_secs)).await;
                }
                Some(20000003) => {
                    println!("   🔇 静音音频  request_id={}（未检测到人声）", &handle.id[..8]);
                    return Ok(TranscriptionOutput {
                        raw_json: serde_json::to_value(&parsed).unwrap_or(Value::Null),
                        text: None,
                    });
                }
                Some(code @ 45000001) => {
                    return Err(anyhow!("请求参数无效 (45000001): request_id={}", &handle.id[..8]));
                }
                Some(code @ 45000002) => {
                    println!("   ⚠️  空音频 (45000002): request_id={}", &handle.id[..8]);
                    return Ok(TranscriptionOutput {
                        raw_json: serde_json::to_value(&parsed).unwrap_or(Value::Null),
                        text: None,
                    });
                }
                Some(code) if (45000100..45000200).contains(&code) => {
                    return Err(anyhow!(
                        "音频无效: 状态码={}, request_id={}",
                        code,
                        &handle.id[..8]
                    ));
                }
                Some(code) if (55000000..56000000).contains(&code) => {
                    return Err(anyhow!(
                        "服务端错误: 状态码={}, request_id={}",
                        code,
                        &handle.id[..8]
                    ));
                }
                Some(other) => {
                    return Err(anyhow!(
                        "未知状态码 {}: request_id={}",
                        other,
                        &handle.id[..8]
                    ));
                }
                None => {
                    if parsed.result.is_some() {
                        println!("   ✅ body 已有结果  request_id={}", &handle.id[..8]);
                        let text = parsed.result.as_ref().and_then(|r| r.text.clone());
                        return Ok(TranscriptionOutput {
                            raw_json: serde_json::to_value(&parsed).unwrap_or(Value::Null),
                            text,
                        });
                    }
                    println!("   ⚠️  状态未知，继续轮询  request_id={}", &handle.id[..8]);
                    sleep(Duration::from_secs(config.poll_interval_secs)).await;
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

        // 保存原始 JSON 响应
        let json_path = result_dir.join("response.json");
        fs::write(&json_path, serde_json::to_vec_pretty(&output.raw_json)?)?;

        // 提取文本
        let extracted_text = output.text.clone().or_else(|| {
            output
                .raw_json
                .get("result")
                .and_then(|r| r.get("utterances"))
                .and_then(|utts| utts.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|u| u.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        });

        if let Some(ref text) = extracted_text {
            let txt_path = result_dir.join("result.txt");
            fs::write(&txt_path, text)?;
            println!("   📝 文本已保存: {}", txt_path.display());
        }

        Ok(SubmittedTaskSummary {
            request_id: handle.id.clone(),
            chunk_path: chunk.path.display().to_string(),
            submission_url: chunk.submission_url.clone().unwrap_or_default(),
            status_code: Some(20000000),
            status_message: Some("OK".to_string()),
            result_text: extracted_text,
            result_json_path: Some(json_path.display().to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// 火山引擎专用工具函数
// ---------------------------------------------------------------------------

fn build_headers(config: &Config, request_id: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    // 兼容简写：seedasr → volc.seedasr.auc，bigasr → volc.bigasr.auc
    let res_id = match config.resource_id.as_str() {
        "seedasr" => "volc.seedasr.auc",
        "bigasr" => "volc.bigasr.auc",
        other => other,
    };
    headers.insert(
        "X-Api-Resource-Id",
        HeaderValue::from_str(res_id)?,
    );
    headers.insert("X-Api-Request-Id", HeaderValue::from_str(request_id)?);
    headers.insert("X-Api-Sequence", HeaderValue::from_static("-1"));

    if config.legacy_mode {
        headers.insert(
            "X-Api-App-Key",
            HeaderValue::from_str(config.app_key.as_deref().unwrap_or_default())?,
        );
        headers.insert(
            "X-Api-Access-Key",
            HeaderValue::from_str(config.access_key.as_deref().unwrap_or_default())?,
        );
    } else {
        headers.insert("X-Api-Key", HeaderValue::from_str(&config.api_key)?);
    }

    Ok(headers)
}

fn parse_status_headers(headers: &HeaderMap) -> ApiStatus {
    let code = headers
        .get("X-Api-Status-Code")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());

    let message = headers
        .get("X-Api-Message")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    ApiStatus { code, message }
}

// ---------------------------------------------------------------------------
// 构建 corpus 对象
// ---------------------------------------------------------------------------

fn build_corpus(config: &Config) -> Option<CorpusBlock> {
    let has_boosting = config.boosting_table_name.as_ref()
        .map(|s| !s.is_empty()).unwrap_or(false);
    let has_correct = config.correct_table_name.as_ref()
        .map(|s| !s.is_empty()).unwrap_or(false);
    let has_context = config.corpus_context.as_ref()
        .map(|s| !s.is_empty()).unwrap_or(false);

    if !has_boosting && !has_correct && !has_context {
        return None;
    }

    Some(CorpusBlock {
        boosting_table_name: config.boosting_table_name.clone(),
        correct_table_name: config.correct_table_name.clone(),
        context: config.corpus_context.clone(),
    })
}
