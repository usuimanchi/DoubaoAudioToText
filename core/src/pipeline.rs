//! 通用编排流程：提交 → 轮询 → 保存 → 合并
//!
//! 本模块不依赖 CLI（clap/dialoguer），可被 CLI 和 Tauri GUI 复用。
//! 所有进度输出通过 `config.reporter` 完成。

use anyhow::Result;
use std::fs;
use std::path::PathBuf;

use crate::backend::{JobHandle, TranscriptionBackend};
use crate::input;
use crate::output;
use crate::types::*;

// ---------------------------------------------------------------------------
// 公共入口
// ---------------------------------------------------------------------------

/// 按提供商分发，运行完整的转写管道。
pub async fn run_pipeline_for_provider(
    client: &reqwest::Client,
    config: &mut Config,
    inputs: &[String],
) -> Result<()> {
    match config.provider {
        Provider::Volcengine => {
            run_pipeline::<crate::volcengine::VolcengineBackend>(client, config, inputs).await
        }
        Provider::Las => {
            run_pipeline::<crate::las::LasBackend>(client, config, inputs).await
        }
        Provider::Ark => {
            run_pipeline::<crate::ark::ArkBackend>(client, config, inputs).await
        }
        Provider::Azure => {
            run_pipeline::<crate::azure::AzureBackend>(client, config, inputs).await
        }
    }
}

// ---------------------------------------------------------------------------
// 核心编排
// ---------------------------------------------------------------------------

pub async fn run_pipeline<B: TranscriptionBackend>(
    client: &reqwest::Client,
    config: &mut Config,
    inputs: &[String],
) -> Result<()> {
    config.reporter.log(format!("🎙️  提供商: {}", B::provider_name()));

    let mut all_summaries: Vec<PersistedSummary> = Vec::new();
    let mut total_submitted = 0usize;
    let mut total_prepared = 0usize;

    for input_str in inputs {
        config.reporter.log("═".repeat(60));
        config.reporter.log(format!("📥  处理输入: {input_str}"));

        // 0) 输出目录：本地文件 → 源目录
        let p = std::path::PathBuf::from(input_str);
        if p.exists() && config.output_dir == std::path::PathBuf::from(DEFAULT_OUTPUT_DIR) {
            if let Some(parent) = p.parent() {
                config.output_dir = parent.to_path_buf();
                config
                    .reporter
                    .log(format!("   📂 输出目录: {}", config.output_dir.display()));
            }
        }

        // 1) 解析输入
        let mut audio_input = input::resolve_input(input_str, &config.output_dir, config.reporter.as_ref()).await?;

        // 2) 准备音频（检查/转换/切分）
        let mut prepared_chunks = crate::audio::prepare_audio(&audio_input, config).await?;
        total_prepared += prepared_chunks.len();

        // 输出片段摘要
        config
            .reporter
            .log(format!("   ┌─ 准备就绪: {} 个片段", prepared_chunks.len()));
        for (i, c) in prepared_chunks.iter().enumerate() {
            let dur = crate::audio::format_duration(c.duration_secs);
            let sz = crate::audio::format_size(c.size_bytes);
            config.reporter.log(format!(
                "   │  [{i}] {dur}  {sz}  格式={} 编码={}",
                c.format, c.codec
            ));
        }

        // 2.5) LAS/Volcengine 本地文件需要上传到 TOS 获取 URL
        if config.provider == Provider::Las || config.provider == Provider::Volcengine {
            if let (Ok(ak), Ok(sk)) = (
                std::env::var("TOS_ACCESS_KEY"),
                std::env::var("TOS_SECRET_KEY"),
            ) {
                if !ak.is_empty() && !sk.is_empty() {
                    if let Ok(uploader) = crate::tos_upload::create_tos_uploader(
                        &ak, &sk, "cn-beijing", "tos-cn-beijing.volces.com", "amamizu-oss",
                    ) {
                        for chunk in &mut prepared_chunks {
                            if chunk.submission_url.is_some() {
                                continue;
                            }
                            let name = chunk
                                .path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("audio.bin");
                            let key = format!("las-audio/{name}");
                            config
                                .reporter
                                .log(format!("   📤 上传到 TOS: {name}"));
                            match uploader.upload(&chunk.path, &key).await {
                                Ok(url) => {
                                    config
                                        .reporter
                                        .log(format!("   │  ✅ {url}"));
                                    chunk.submission_url = Some(url);
                                }
                                Err(e) => config
                                    .reporter
                                    .warn(format!("   │  ⚠️  {e}")),
                            }
                        }
                    }
                }
            }
        }

        // 3) 筛选可提交片段
        let submittable: Vec<&PreparedChunk> = if config.provider == Provider::Ark {
            prepared_chunks.iter().collect()
        } else {
            prepared_chunks
                .iter()
                .filter(|c| c.submission_url.is_some())
                .collect()
        };

        let mut submitted_summaries: Vec<SubmittedTaskSummary> = Vec::new();

        if submittable.is_empty() {
            if !audio_input.is_url {
                config.reporter.warn("   ⚠️  本地文件无可提交的 URL。".to_string());
                config
                    .reporter
                    .log("   💡 请使用 Ark 提供商（默认），或设置 TOS_ACCESS_KEY / TOS_SECRET_KEY 环境变量。".to_string());
            } else {
                config.reporter.warn("   ⚠️  该输入为 URL，但音频需要转换/切分，无法用于提交已处理的本地副本。".to_string());
                config
                    .reporter
                    .log("   💡 建议使用 Ark 提供商（默认），通过 Files API 直接提交。".to_string());
            }
        } else if config.prepare_only {
            config.reporter.log(format!(
                "   ⏭️  --prepare-only 模式，跳过 API 提交（共 {} 个可提交片段）。",
                submittable.len()
            ));
        } else {
            // 4) 批量提交
            config.reporter.log(format!(
                "   ┌─ 开始提交 {} 个任务...",
                submittable.len()
            ));
            let mut handles: Vec<JobHandle> = Vec::new();
            for chunk in &submittable {
                match B::submit(client, config, chunk).await {
                    Ok(handle) => {
                        handles.push(handle);
                    }
                    Err(e) => {
                        config
                            .reporter
                            .error(format!("   │  ❌ 提交失败: {e}"));
                    }
                }
            }
            total_submitted += handles.len();

            // 5) 等待完成并保存结果
            for (handle, chunk) in handles.iter().zip(submittable.iter()) {
                match B::wait_for_completion(client, config, handle).await {
                    Ok(output) => match B::save_result(config, handle, &output, chunk) {
                        Ok(summary) => submitted_summaries.push(summary),
                        Err(e) => {
                            config
                                .reporter
                                .error(format!("   ❌ 保存结果失败: {e}"));
                            submitted_summaries.push(SubmittedTaskSummary {
                                request_id: handle.id.clone(),
                                chunk_path: chunk.path.display().to_string(),
                                submission_url: chunk
                                    .submission_url
                                    .clone()
                                    .unwrap_or_default(),
                                status_code: None,
                                status_message: Some(format!("{e}")),
                                result_text: None,
                                result_json_path: None,
                            });
                        }
                    },
                    Err(e) => {
                        config
                            .reporter
                            .error(format!("   ❌ 任务失败: {e}"));
                        submitted_summaries.push(SubmittedTaskSummary {
                            request_id: handle.id.clone(),
                            chunk_path: chunk.path.display().to_string(),
                            submission_url: chunk
                                .submission_url
                                .clone()
                                .unwrap_or_default(),
                            status_code: None,
                            status_message: Some(format!("{e}")),
                            result_text: None,
                            result_json_path: None,
                        });
                    }
                }
            }
        }

        // 6) 汇总
        let chunk_summaries: Vec<ChunkSummary> = prepared_chunks
            .iter()
            .map(|c| ChunkSummary {
                path: c.path.display().to_string(),
                format: c.format.clone(),
                codec: c.codec.clone(),
                sample_rate: c.sample_rate,
                duration_secs: c.duration_secs,
                size_bytes: c.size_bytes,
            })
            .collect();

        all_summaries.push(PersistedSummary {
            original_input: input_str.clone(),
            source_path: audio_input.source_path.display().to_string(),
            is_url: audio_input.is_url,
            chunks: chunk_summaries,
            submitted: submitted_summaries,
        });
    }

    // 输出结果（单片段直接取，多片段合并去重）
    for summary in &all_summaries {
        let text = if summary.submitted.len() <= 1 {
            summary
                .submitted
                .first()
                .and_then(|s| s.result_text.clone())
                .unwrap_or_default()
        } else {
            merge_chunk_results(summary).unwrap_or_default()
        };

        if !text.is_empty() {
            let stem = output_stem(&summary.original_input);
            let out_path = config
                .output_dir
                .join(format!("result_{stem}.txt"));
            let count = text.chars().count();
            fs::write(&out_path, &text)?;
            config
                .reporter
                .log(format!("   📝 结果已保存: {}（{} 字）", out_path.display(), count));
        }
    }

    // 写入 manifest
    if !all_summaries.is_empty() {
        let manifest_path = config.output_dir.join("manifest.json");
        output::write_manifest(&manifest_path, &all_summaries, config)?;
    }

    config.reporter.log(format!(
        "\n🎉 全部完成！准备: {total_prepared} 个片段，提交: {total_submitted} 个任务。"
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// 多片段合并与去重
// ---------------------------------------------------------------------------

pub fn merge_chunk_results(summary: &PersistedSummary) -> Result<String> {
    let texts: Vec<String> = summary
        .submitted
        .iter()
        .map(|s| {
            if let Some(ref t) = s.result_text {
                return t.clone();
            }
            if let Some(ref p) = s.result_json_path {
                if let Ok(raw) = std::fs::read_to_string(p) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if let Some(t) = crate::ark::extract_text_from_response(&val) {
                            return t;
                        }
                    }
                }
            }
            String::new()
        })
        .collect();

    if texts.is_empty() {
        return Ok(String::new());
    }
    if texts.len() == 1 {
        return Ok(texts[0].clone());
    }

    let mut merged = texts[0].clone();
    for i in 1..texts.len() {
        let (trimmed, overlap) = dedup_overlap(&merged, &texts[i]);
        merged = trimmed;
        merged.push_str("\n\n");
        merged.push_str(&texts[i][overlap..]);
    }

    Ok(merged)
}

/// 找两段文本的重叠部分。返回 (trimmed_prev, overlap_len)。
pub fn dedup_overlap(prev: &str, next: &str) -> (String, usize) {
    let tail = prev
        .chars()
        .rev()
        .take(150)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let head: String = next.chars().take(150).collect();

    if tail.is_empty() || head.is_empty() {
        return (prev.to_string(), 0);
    }

    let mut best = 0usize;
    for len in (10..=tail.chars().count().min(head.chars().count())).rev() {
        let tail_suffix: String = tail
            .chars()
            .rev()
            .take(len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let head_prefix: String = head.chars().take(len).collect();
        if tail_suffix == head_prefix {
            best = len;
            break;
        }
    }

    if best < 15 {
        return (prev.to_string(), 0);
    }

    let without_overlap: String = prev
        .chars()
        .take(prev.chars().count() - best)
        .collect();
    (without_overlap, best)
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 安全化文件名（替换非法字符为 `_`）
pub fn sanitize_filename(name: &str) -> String {
    const ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\0'];
    name.chars()
        .map(|c| {
            if ILLEGAL.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// 安全化路径（按 `/` 分割后各段过 sanitize_filename）
pub fn sanitize_path(path: &str) -> String {
    path.split('/')
        .map(sanitize_filename)
        .collect::<Vec<_>>()
        .join("/")
}

/// 从输入 URL/路径中提取干净的文件名（不含扩展名）
pub fn output_stem(input: &str) -> String {
    let name = input
        .rsplit('/')
        .next()
        .unwrap_or(input)
        .rsplit('?')
        .next()
        .unwrap_or("output");
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    sanitize_filename(stem)
}

/// 检测系统语言（返回 "zh" / "en" / "fr" 等）
pub fn detect_system_lang() -> &'static str {
    if let Ok(locale) = std::env::var("LANG") {
        let l = locale.to_lowercase();
        if l.starts_with("fr") || l.starts_with("fr_") {
            return "fr";
        }
        if l.starts_with("en") || l.starts_with("en_") {
            return "en";
        }
        if l.starts_with("zh") || l.starts_with("zh_") {
            return "zh";
        }
    }
    "zh"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_overlap_no_overlap() {
        let (trimmed, len) = dedup_overlap("第一段内容结束。", "第二段内容开始。");
        assert_eq!(len, 0);
        assert_eq!(trimmed, "第一段内容结束。");
    }

    #[test]
    fn test_sanitize_filename_removes_illegal() {
        let clean = sanitize_filename("test:file?name");
        assert!(!clean.contains(':'));
        assert!(!clean.contains('?'));
    }

    #[test]
    fn test_output_stem_url() {
        let stem = output_stem("https://example.com/path/to/audio.mp3?token=abc");
        assert_eq!(stem, "audio");
    }
}
