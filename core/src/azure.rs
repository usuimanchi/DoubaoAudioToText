//! Azure Speech-to-Text 转录后端
//!
//! 使用 Azure Cognitive Services Speech-to-Text REST API (2025-10-15)。
//! 支持：
//! - 批量转录提交与轮询
//! - 多语言识别（Language Identification，最多 10 种候选语言）
//! - 说话人分类（diarization）
//! - 结果中的 per-phrase locale 标注（混语场景的核心优势）

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;
use tokio::time::sleep;

use crate::backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
use crate::types::{Config, PreparedChunk, SubmittedTaskSummary, Provider};

// ---------------------------------------------------------------------------
// Azure API 请求类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct AzureSubmitRequest {
    contentUrls: Vec<String>,
    locale: String,
    displayName: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<AzureProperties>,
}

#[derive(Debug, Clone, Serialize)]
struct AzureProperties {
    #[serde(skip_serializing_if = "Option::is_none")]
    wordLevelTimestampsEnabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarizationEnabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diarization: Option<AzureDiarization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    languageIdentification: Option<AzureLanguageId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profanityFilterMode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    punctuationMode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channels: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize)]
struct AzureDiarization {
    #[serde(skip_serializing_if = "Option::is_none")]
    minCount: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    maxCount: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct AzureLanguageId {
    candidateLocales: Vec<String>,
}

// ---------------------------------------------------------------------------
// Azure API 响应类型
// ---------------------------------------------------------------------------

/// 创建转录任务后的 201 响应
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzureTranscription {
    #[serde(default)]
    #[serde(rename = "self")]
    self_url: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    created_date_time: Option<String>,
    #[serde(default)]
    last_action_date_time: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    links: Option<AzureLinks>,
    #[serde(default)]
    properties: Option<Value>,
    #[serde(default)]
    error: Option<AzureErrorBody>,
}

#[derive(Debug, Deserialize)]
struct AzureLinks {
    #[serde(default)]
    files: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AzureErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// 文件列表响应
#[derive(Debug, Deserialize)]
struct AzureFilesList {
    #[serde(default)]
    values: Vec<AzureFileEntry>,
}

#[derive(Debug, Deserialize)]
struct AzureFileEntry {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    links: Option<AzureFileLinks>,
}

#[derive(Debug, Deserialize)]
struct AzureFileLinks {
    #[serde(default)]
    contentUrl: Option<String>,
}

/// 结果文件 JSON（从 SAS URL 下载）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzureResultFile {
    #[serde(default)]
    combined_recognized_phrases: Vec<AzureRecognizedPhrase>,
    #[serde(default)]
    recognized_phrases: Vec<AzureRecognizedPhrase>,
    #[serde(default)]
    duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AzureRecognizedPhrase {
    #[serde(default)]
    channel: Option<u32>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    lexical: Option<String>,
    #[serde(default)]
    itn: Option<String>,
    #[serde(default)]
    masked_itn: Option<String>,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    speaker: Option<u32>,
}

// ---------------------------------------------------------------------------
// Azure Backend
// ---------------------------------------------------------------------------

pub struct AzureBackend;

#[async_trait]
impl TranscriptionBackend for AzureBackend {
    fn provider_name() -> &'static str {
        "Azure Speech-to-Text"
    }

    async fn submit(
        client: &Client,
        config: &Config,
        chunk: &PreparedChunk,
    ) -> Result<JobHandle> {
        let file_url = chunk.submission_url.as_ref()
            .ok_or_else(|| anyhow!("Azure: 片段 {} 没有提交 URL", chunk.path.display()))?;

        let region = config.azure_region.as_deref()
            .ok_or_else(|| anyhow!("Azure region 未设置，请使用 --azure-region 指定"))?;

        let azure_key = config.azure_key.as_deref()
            .ok_or_else(|| anyhow!("Azure subscription key 未设置，请使用 --azure-key 指定"))?;

        let endpoint = format!(
            "https://{}.api.cognitive.microsoft.com/speechtotext/transcriptions:submit?api-version=2025-10-15",
            region
        );

        // ---- 构建 properties ----
        let mut props = AzureProperties {
            wordLevelTimestampsEnabled: if config.word_level_timestamps { Some(true) } else { None },
            diarizationEnabled: if config.enable_speaker_info { Some(true) } else { None },
            diarization: None,
            languageIdentification: None,
            profanityFilterMode: Some(config.profanity_filter_mode.clone()),
            punctuationMode: Some(config.punctuation_mode.clone()),
            channels: None,
        };

        if config.enable_speaker_info {
            props.diarization = Some(AzureDiarization {
                minCount: Some(1),
                maxCount: Some(10),
            });
        }

        // ---- locale：单语言 vs 多语言识别 ----
        let locale = if let Some(ref candidates) = config.candidate_locales {
            if candidates.len() > 10 {
                return Err(anyhow!("Azure 候选语言最多 10 个，当前提供了 {} 个", candidates.len()));
            }
            props.languageIdentification = Some(AzureLanguageId {
                candidateLocales: candidates.clone(),
            });
            // locale 字段设为第一个候选语言
            candidates.first().cloned().unwrap_or_else(|| "en-US".to_string())
        } else {
            config.language.clone().unwrap_or_else(|| "zh-CN".to_string())
        };

        let display_name = chunk
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio_chunk")
            .to_string();

        let body = AzureSubmitRequest {
            contentUrls: vec![file_url.to_string()],
            locale,
            displayName: display_name,
            description: Some(format!("Submitted by volc_auc_batch_client")),
            properties: Some(props),
        };

        let resp = client
            .post(&endpoint)
            .header("Ocp-Apim-Subscription-Key", azure_key)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("Azure 提交任务网络请求失败")?;

        let status_code = resp.status();
        if !status_code.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Azure 提交失败: HTTP {} — {}",
                status_code,
                &err_body[..err_body.len().min(500)]
            ));
        }

        // 解析 201 响应，从 `self` URL 中提取 job ID
        let body_text = resp.text().await.context("读取 Azure 响应 body 失败")?;

        // Azure 201 响应格式：直接是一个 JSON 对象，有 self 字段
        let raw: Value = serde_json::from_str(&body_text)
            .context("解析 Azure 提交响应 JSON 失败")?;

        let self_url = raw
            .get("self")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Azure 响应中没有 'self' 链接"))?;

        // self URL 格式:
        //   https://{region}.api.cognitive.microsoft.com/speechtotext/transcriptions/{jobId}
        let job_id = self_url
            .rsplit('/')
            .next()
            .ok_or_else(|| anyhow!("无法从 self URL 中解析 job ID: {self_url}"))?;

        println!("   ✅ Azure 任务已提交  job_id={}", job_id);

        Ok(JobHandle {
            id: job_id.to_string(),
            query_url: Some(self_url.to_string()),
            provider: Provider::Azure,
            operator_version: None,
        })
    }

    async fn wait_for_completion(
        client: &Client,
        config: &Config,
        handle: &JobHandle,
    ) -> Result<TranscriptionOutput> {
        let azure_key = config.azure_key.as_deref().unwrap_or("");
        let poll_interval = Duration::from_secs(config.poll_interval_secs);
        let start = std::time::Instant::now();
        let mut tries = 0u32;

        let query_url = handle.query_url.as_ref()
            .map(|u| format!("{u}?api-version=2025-10-15"))
            .unwrap_or_else(|| {
                let region = config.azure_region.as_deref().unwrap_or("eastasia");
                format!(
                    "https://{}.api.cognitive.microsoft.com/speechtotext/transcriptions/{}?api-version=2025-10-15",
                    region, handle.id
                )
            });

        loop {
            tries += 1;

            let resp = client
                .get(&query_url)
                .header("Ocp-Apim-Subscription-Key", azure_key)
                .send()
                .await
                .context("Azure 查询任务状态失败")?;

            if !resp.status().is_success() {
                let http_status = resp.status();
                let err_body = resp.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "Azure 查询失败 HTTP {}: {}",
                    http_status,
                    &err_body[..err_body.len().min(500)]
                ));
            }

            let body_text = resp.text().await.context("读取 Azure 查询响应失败")?;
            let raw: Value = serde_json::from_str(&body_text)
                .context("解析 Azure 查询响应 JSON 失败")?;

            let status = raw
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");

            match status {
                "Succeeded" => {
                    let elapsed = start.elapsed();
                    println!(
                        "   ✅ Azure 任务完成  job_id={}  耗时={:.0}s",
                        &handle.id[..16],
                        elapsed.as_secs()
                    );

                    // 获取 files 链接
                    let files_url = raw
                        .get("links")
                        .and_then(|l| l.get("files"))
                        .and_then(|f| f.as_str())
                        .ok_or_else(|| anyhow!("Azure 响应中没有 files 链接"))?;

                    return download_and_extract_results(client, azure_key, files_url, handle).await;
                }
                "Failed" => {
                    let error_info = raw.get("error").and_then(|e| {
                        let code = e.get("code").and_then(|c| c.as_str()).unwrap_or("?");
                        let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("?");
                        Some(format!("{code}: {msg}"))
                    }).unwrap_or_else(|| "Unknown error".to_string());
                    return Err(anyhow!("Azure 任务失败: {}", error_info));
                }
                "NotStarted" | "Running" => {
                    if tries % 12 == 1 {
                        let elapsed = start.elapsed();
                        println!(
                            "   ⏳ 等待中  job_id={}  已等待 {:.0}s  status={}",
                            &handle.id[..16],
                            elapsed.as_secs(),
                            status
                        );
                    }
                    sleep(poll_interval).await;
                }
                other => {
                    return Err(anyhow!(
                        "Azure 未知状态 '{}' (job_id={})",
                        other,
                        &handle.id[..16]
                    ));
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

        // 保存原始 JSON
        let json_path = result_dir.join("response.json");
        fs::write(&json_path, serde_json::to_vec_pretty(&output.raw_json)?)?;

        // 保存纯文本
        if let Some(ref text) = output.text {
            let txt_path = result_dir.join("result.txt");
            fs::write(&txt_path, text)?;
            println!("   📝 文本已保存: {}", txt_path.display());
        }

        // 多语言映射
        if let Some(locale_map) = extract_locale_map(&output.raw_json) {
            let lang_path = result_dir.join("language_map.json");
            fs::write(&lang_path, serde_json::to_vec_pretty(&locale_map)?)?;
            println!("   🌐 语言分布已保存: {}", lang_path.display());
        }

        Ok(SubmittedTaskSummary {
            request_id: handle.id.clone(),
            chunk_path: chunk.path.display().to_string(),
            submission_url: chunk.submission_url.clone().unwrap_or_default(),
            status_code: Some(200),
            status_message: Some("Succeeded".to_string()),
            result_text: output.text.clone(),
            result_json_path: Some(json_path.display().to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// Azure 结果下载与文本提取
// ---------------------------------------------------------------------------

async fn download_and_extract_results(
    client: &Client,
    azure_key: &str,
    files_url: &str,
    handle: &JobHandle,
) -> Result<TranscriptionOutput> {
    // 1) 获取文件列表
    let files_resp = client
        .get(files_url)
        .header("Ocp-Apim-Subscription-Key", azure_key)
        .send()
        .await
        .context("获取 Azure 文件列表失败")?;

    let files_body: AzureFilesList = files_resp
        .json()
        .await
        .context("解析 Azure 文件列表失败")?;

    // 2) 找到 Transcription 类型的文件
    let transcription_files: Vec<&AzureFileEntry> = files_body
        .values
        .iter()
        .filter(|f| f.kind.as_deref() == Some("Transcription"))
        .collect();

    if transcription_files.is_empty() {
        return Err(anyhow!("Azure 文件列表中没有 Transcription 类型的文件"));
    }

    // 3) 下载每个 Transcription 文件的结果 JSON
    let mut all_phrases: Vec<Value> = Vec::new();
    let mut all_raw = Vec::new();

    for entry in &transcription_files {
        let content_url = entry
            .links
            .as_ref()
            .and_then(|l| l.contentUrl.as_ref())
            .ok_or_else(|| {
                anyhow!(
                    "Azure 文件 {} 没有 contentUrl",
                    entry.name.as_deref().unwrap_or("?")
                )
            })?;

        // SAS URL，无需 auth header
        let result_resp = client
            .get(content_url)
            .send()
            .await
            .context("下载 Azure 结果文件失败")?;

        let result_raw: Value = result_resp
            .json()
            .await
            .context("解析 Azure 结果文件 JSON 失败")?;

        all_raw.push(result_raw.clone());

        // 提取短语
        if let Some(phrases) = result_raw.get("combinedRecognizedPhrases").and_then(|p| p.as_array()) {
            for phrase in phrases {
                all_phrases.push(phrase.clone());
            }
        } else if let Some(phrases) = result_raw.get("recognizedPhrases").and_then(|p| p.as_array()) {
            for phrase in phrases {
                all_phrases.push(phrase.clone());
            }
        }
    }

    // 4) 提取文本：优先 display（带标点/ITN），其次 lexical
    let text_lines: Vec<String> = all_phrases
        .iter()
        .filter_map(|p| {
            p.get("display")
                .or_else(|| p.get("lexical"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    let text = if text_lines.is_empty() {
        None
    } else {
        Some(text_lines.join("\n"))
    };

    // 5) 构建输出
    let raw_json = if all_raw.len() == 1 {
        all_raw.into_iter().next().unwrap_or(Value::Null)
    } else {
        // 多个文件时，构建一个合并 JSON
        json!({
            "summary": {
                "jobId": handle.id,
                "provider": "azure",
                "numFiles": all_raw.len(),
            },
            "combinedPhrases": all_phrases,
        })
    };

    Ok(TranscriptionOutput {
        raw_json,
        text,
    })
}

// ---------------------------------------------------------------------------
// 多语言标注提取
// ---------------------------------------------------------------------------

/// 从结果 JSON 中提取每句话的语言标注（仅在启用 languageIdentification 时有数据）
fn extract_locale_map(raw: &Value) -> Option<Value> {
    let phrases = raw
        .get("combinedRecognizedPhrases")
        .or_else(|| raw.get("recognizedPhrases"))
        .and_then(|p| p.as_array())?;

    let entries: Vec<Value> = phrases
        .iter()
        .filter_map(|p| {
            let locale = p.get("locale")?.as_str()?;
            let text = p
                .get("display")
                .or_else(|| p.get("lexical"))
                .and_then(|t| t.as_str())?;
            let confidence = p.get("confidence").and_then(|c| c.as_f64());
            let speaker = p.get("speaker").and_then(|s| s.as_u64());

            let mut entry = json!({
                "locale": locale,
                "text": text,
            });
            if let Some(conf) = confidence {
                entry["confidence"] = json!(conf);
            }
            if let Some(spk) = speaker {
                entry["speaker"] = json!(spk);
            }
            Some(entry)
        })
        .collect();

    if entries.is_empty() {
        None
    } else {
        // 统计语言分布
        let mut lang_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for e in &entries {
            if let Some(loc) = e.get("locale").and_then(|l| l.as_str()) {
                *lang_counts.entry(loc.to_string()).or_insert(0) += 1;
            }
        }

        Some(json!({
            "entries": entries,
            "summary": lang_counts,
        }))
    }
}
