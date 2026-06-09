//! Ark 平台（方舟）多模态大模型音频理解后端
//!
//! 使用 doubao-seed-2-0-lite 的 Responses API 进行音频转写。
//! 特点：
//! - 同步 API，无需轮询
//! - 中文标点 + 法语原文保留
//! - 中法自然分段
//! - 支持 MP3/WAV/AAC/FLAC/M4A/AMR
//! - URL 方式：≤ 25MB，≤ 120 分钟

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::time::Instant;

use crate::backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
use crate::types::{Config, PreparedChunk, SubmittedTaskSummary, Provider};

const ARK_BASE: &str = "https://ark.cn-beijing.volces.com/api/v3";
const ARK_MODEL: &str = "doubao-seed-2-0-lite-260428";

const DEFAULT_PROMPT: &str = "\
你是一个专业的多语种语音转写助手。请严格遵循以下规则转写这段音频：

1. **法语部分**：必须保留法语原文，逐词逐句准确转写，绝对不能翻译、不能意译、不能改写成其他语言。
2. **中文部分**：准确转写，并添加正确的标点符号（句号、逗号、问号等）。
3. **中法混合**：按照说话人实际使用的语言分别记录，不要混在同一段。如果一段话中同时包含中文和法语，请分开分行记录。
4. **格式**：每个独立的语句或自然停顿处换行。如有明显的话题切换，用空行分隔。
5. **不要添加任何解释、评论、总结或元数据**（如'这段说的是...'、'音频内容为...'）。只输出转写文本。";

// ---------------------------------------------------------------------------
// Ark 请求/响应
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ArkRequest {
    model: String,
    input: Vec<ArkMessage>,
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
    audio_url: Option<String>,
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
        let file_url = chunk
            .submission_url
            .as_ref()
            .ok_or_else(|| anyhow!("Ark: 片段 {} 没有提交 URL", chunk.path.display()))?;

        let prompt = config
            .corpus_context
            .as_deref()
            .unwrap_or(DEFAULT_PROMPT);

        let request = ArkRequest {
            model: ARK_MODEL.to_string(),
            input: vec![ArkMessage {
                role: "user".to_string(),
                content: vec![
                    ArkContent {
                        content_type: "input_audio".to_string(),
                        audio_url: Some(file_url.clone()),
                        text: None,
                    },
                    ArkContent {
                        content_type: "input_text".to_string(),
                        audio_url: None,
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

            // SRT 字幕（Ark 不返回 utterances，从文本推算简单时间轴）
            let srt = build_simple_srt(t);
            let srt_path = result_dir.join("result.srt");
            fs::write(&srt_path, &srt)?;

            // 格式化文本（语言分段）
            let formatted = build_formatted_text(t);
            let fmt_path = result_dir.join("result_formatted.md");
            fs::write(&fmt_path, &formatted)?;
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

// ---------------------------------------------------------------------------
// 后处理
// ---------------------------------------------------------------------------

fn build_simple_srt(text: &str) -> String {
    let paragraphs: Vec<&str> = text.split("\n\n").filter(|p| !p.trim().is_empty()).collect();
    let mut srt = String::new();
    let secs_per_para = 15u64; // rough estimate
    for (i, p) in paragraphs.iter().enumerate() {
        let start = i as u64 * secs_per_para;
        let end = start + secs_per_para;
        let h = |ms: u64| format!("{:02}:{:02}:{:02},000", ms / 3600, (ms % 3600) / 60, ms % 60);
        srt.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, h(start), h(end), p.trim()));
    }
    srt
}

fn build_formatted_text(text: &str) -> String {
    let mut out = String::from("# 转录结果\n\n");
    let paragraphs: Vec<&str> = text.split("\n\n").filter(|p| !p.trim().is_empty()).collect();
    for p in paragraphs {
        let lang = detect_lang(p);
        let flag = if lang == "zh" { "🇨🇳 中文" } else { "🇫🇷 Français" };
        out.push_str(&format!("### {}\n\n{}\n\n---\n\n", flag, p.trim()));
    }
    out
}

fn detect_lang(text: &str) -> &'static str {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    for c in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3000}'..='\u{303f}').contains(&c) || ('\u{ff00}'..='\u{ffef}').contains(&c) {
            cjk += 1;
        } else if c.is_ascii_alphabetic() || "àâäéèêëîïôöùûüçœæÀÂÄÉÈÊËÎÏÔÖÙÛÜÇŒÆ".contains(c) {
            latin += 1;
        }
    }
    if cjk > latin { "zh" } else { "fr" }
}
