//! Ark 平台（方舟）多模态大模型音频理解后端
//!
//! 使用 doubao-seed-2-0-lite 的 Responses API 进行音频转写。
//! 特点：
//! - 同步 API，无需轮询
//! - 通过 Files API 上传音频获取 file_id（最大 512MB）
//! - temperature=0 确保转写确定性，thinking=disabled 省钱提速
//! - 中文标点 + 多语种原文保留
//! - 中法自然分段

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
use crate::types::{Config, PreparedChunk, SubmittedTaskSummary, Provider};

const ARK_BASE: &str = "https://ark.cn-beijing.volces.com/api/v3";

const DEFAULT_PROMPT: &str = "\
你是一个专业的多语种语音转写助手。请严格遵循以下规则转写这段音频：

1. **法语部分**：必须保留法语原文，逐词逐句准确转写，绝对不能翻译、不能意译、不能改写成其他语言。
2. **中文部分**：准确转写，并添加正确的标点符号（句号、逗号、问号等）。
3. **中法混合**：按照说话人实际使用的语言分别记录，不要混在同一段。如果一段话中同时包含中文和法语，请分开分行记录。
4. **格式**：每个独立的语句或自然停顿处换行。如有明显的话题切换，用空行分隔。
5. **不要添加任何解释、评论、总结或元数据**（如'这段说的是...'、'音频内容为...'）。只输出转写文本。

以上规则以法语为例，法语、英语、德语、日语等其他非中文语言均类似。";

// ---------------------------------------------------------------------------
// Ark 请求/响应
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ArkRequest {
    model: String,
    input: Vec<ArkMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

#[derive(Debug, Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    thinking_type: String,
}

#[derive(Debug, Serialize)]
struct ArkMessage {
    role: String,
    content: Vec<ArkContent>,
}

#[derive(Debug, Serialize)]
struct ArkContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Ark Backend
// ---------------------------------------------------------------------------

pub struct ArkBackend;

/// 从 Ark 响应 JSON 中提取文本（公开，供合并逻辑使用）
pub fn extract_text_from_response(raw: &Value) -> Option<String> {
    raw.get("output")?
        .as_array()?
        .iter()
        .find(|o| o.get("type").and_then(|t| t.as_str()) == Some("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// 通过 Files API 上传本地音频文件，返回 file_id
async fn upload_to_files_api(client: &Client, api_key: &str, file_path: &Path) -> Result<String> {
    let file_bytes = fs::read(file_path)
        .with_context(|| format!("读取文件失败: {}", file_path.display()))?;

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.mp3");

    let mime = match file_path.extension().and_then(|e| e.to_str()) {
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/m4a",
        Some("aac") => "audio/aac",
        _ => "audio/mpeg",
    };

    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(file_name.to_string())
        .mime_str(mime)?;

    let form = reqwest::multipart::Form::new()
        .text("purpose", "user_data")
        .part("file", part);

    let resp = client
        .post(&format!("{ARK_BASE}/files"))
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
        .context("Files API 上传失败")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Files API 失败 HTTP {}: {}",
            status,
            &body[..body.len().min(300)]
        ));
    }

    let json: Value = resp.json().await.context("解析 Files API 响应失败")?;
    let file_id = json
        .get("id")
        .and_then(|i| i.as_str())
        .ok_or_else(|| anyhow!("Files API 未返回 file_id: {}", json))?
        .to_string();

    println!("   │  ✅ file_id: {}", file_id);
    Ok(file_id)
}

#[async_trait]
impl TranscriptionBackend for ArkBackend {
    fn provider_name() -> &'static str {
        "Ark 方舟（doubao-seed-2-0-lite）"
    }

    async fn submit(
        client: &Client,
        config: &Config,
        chunk: &PreparedChunk,
    ) -> Result<JobHandle> {
        // 通过 Files API 上传音频，获取 file_id
        println!("   📤 上传到 Files API: {}", chunk.path.display());
        let file_id = upload_to_files_api(client, &config.api_key, &chunk.path).await?;

        let prompt = config
            .corpus_context
            .as_deref()
            .unwrap_or(DEFAULT_PROMPT);

        let request = ArkRequest {
            model: config.ark_model.clone(),
            temperature: Some(0.0),
            thinking: Some(ThinkingConfig {
                thinking_type: "disabled".to_string(),
            }),
            input: vec![ArkMessage {
                role: "user".to_string(),
                content: vec![
                    ArkContent {
                        content_type: "input_audio".to_string(),
                        file_id: Some(file_id),
                        text: None,
                    },
                    ArkContent {
                        content_type: "input_text".to_string(),
                        file_id: None,
                        text: Some(prompt.to_string()),
                    },
                ],
            }],
        };

        let start = Instant::now();
        let resp = client
            .post(&format!("{ARK_BASE}/responses"))
            .header("Authorization", format!("Bearer {}", &config.api_key))
            .header(CONTENT_TYPE, "application/json")
            .json(&request)
            .send()
            .await
            .context("Ark 请求失败")?;

        if !resp.status().is_success() {
            let http_status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Ark 失败 HTTP {}: {}",
                http_status,
                &err_body[..err_body.len().min(500)]
            ));
        }

        let raw: Value = resp.json().await.context("解析 Ark 响应失败")?;

        let text = extract_text_from_response(&raw);
        let elapsed = start.elapsed();

        let preview = text.as_deref().unwrap_or("（无文本）");
        let preview: String = preview.chars().take(80).collect();
        let preview = if preview.len() < text.as_deref().unwrap_or("").len() {
            format!("{preview}...")
        } else {
            preview
        };

        let usage = raw
            .get("usage")
            .and_then(|u| u.get("total_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        println!(
            "   ✅ Ark 完成  耗时={:.0}s  tokens={}  结果: {}",
            elapsed.as_secs(),
            usage,
            preview
        );

        // Ark 是同步 API，submit 即完成，直接把结果存进去
        let handle_id = raw
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or("ark-unknown")
            .to_string();

        // 持久化结果（同步完成，无需 wait）
        let result_dir = config.output_dir.join("results").join(&handle_id);
        fs::create_dir_all(&result_dir)?;

        let json_path = result_dir.join("response.json");
        fs::write(&json_path, serde_json::to_vec_pretty(&raw)?)?;

        if let Some(ref t) = text {
            let txt_path = result_dir.join("result.txt");
            fs::write(&txt_path, t)?;
            println!("   📝 文本已保存: {}", txt_path.display());
        }

        Ok(JobHandle {
            id: handle_id,
            query_url: None,
            provider: Provider::Ark,
            operator_version: None,
        })
    }

    /// Ark 同步完成，submit 时已保存结果，这里直接返回
    async fn wait_for_completion(
        _client: &Client,
        config: &Config,
        handle: &JobHandle,
    ) -> Result<TranscriptionOutput> {
        // 从已保存的文件回读（Ark submit 时已同步完成）
        let json_path = config
            .output_dir
            .join("results")
            .join(&handle.id)
            .join("response.json");
        let raw: Value = serde_json::from_str(
            &fs::read_to_string(&json_path)
                .context("读取 Ark 结果文件失败")?,
        )?;
        let text = extract_text_from_response(&raw);
        Ok(TranscriptionOutput { raw_json: raw, text })
    }

    fn save_result(
        config: &Config,
        handle: &JobHandle,
        output: &TranscriptionOutput,
        chunk: &PreparedChunk,
    ) -> Result<SubmittedTaskSummary> {
        // submit 阶段已保存，此处仅生成 summary
        let json_path = config
            .output_dir
            .join("results")
            .join(&handle.id)
            .join("response.json");

        Ok(SubmittedTaskSummary {
            request_id: handle.id.clone(),
            chunk_path: chunk.path.display().to_string(),
            submission_url: chunk.submission_url.clone().unwrap_or_default(),
            status_code: Some(200),
            status_message: Some("completed".to_string()),
            result_text: output.text.clone(),
            result_json_path: Some(json_path.display().to_string()),
        })
    }
}
