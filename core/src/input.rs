//! 输入处理：解析、展开列表、下载音频输入
//!
//! 注意：交互式 stdin 收集逻辑在 `cli/src/main.rs`，不在此模块中。

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::progress::ProgressReporter;
use crate::types::AudioInput;

// ---------------------------------------------------------------------------
// 展开输入列表（.txt / .list 文件）
// ---------------------------------------------------------------------------

/// 从 Vec<String> 中展开 .txt/.list 文件内容，非交互。
pub fn expand_input_list(inputs: Vec<String>, reporter: &dyn ProgressReporter) -> Result<Vec<String>> {
    if inputs.is_empty() {
        return Err(anyhow!("输入列表为空"));
    }
    let mut expanded = Vec::new();
    for inp in inputs {
        let trimmed = inp.trim();
        if trimmed.is_empty() {
            continue;
        }
        let p = PathBuf::from(trimmed);
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("list"))
            .unwrap_or(false)
            && p.exists()
        {
            let content =
                fs::read_to_string(&p).with_context(|| format!("无法读取输入列表文件: {}", p.display()))?;
            let lines: Vec<String> = content
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            reporter.log(format!("从 {} 展开 {} 个输入", p.display(), lines.len()));
            expanded.extend(lines);
        } else {
            expanded.push(trimmed.to_string());
        }
    }
    reporter.log(format!("共 {} 个音频输入", expanded.len()));
    Ok(expanded)
}

// ---------------------------------------------------------------------------
// 输入解析：URL → 下载；本地路径 → 直接使用
// ---------------------------------------------------------------------------

pub async fn resolve_input(input: &str, out_dir: &Path, reporter: &dyn ProgressReporter) -> Result<AudioInput> {
    // HTTP(S) URL
    if input.starts_with("http://") || input.starts_with("https://") {
        let file_name = sanitize_filename_from_url(input);
        let dl_dir = out_dir.join("download");
        fs::create_dir_all(&dl_dir)?;
        let dst = dl_dir.join(&file_name);

        // 避免重复下载
        if dst.exists() {
            let meta = fs::metadata(&dst)?;
            if meta.len() > 0 {
                reporter.log(format!("   ⏭️  文件已存在，跳过下载: {}", dst.display()));
                return Ok(AudioInput {
                    original: input.to_string(),
                    source_path: dst,
                    is_url: true,
                    submission_url: Some(input.to_string()),
                });
            }
        }

        reporter.log(format!("   ⬇️  正在下载: {input}"));
        download_url(input, &dst, reporter).await?;
        return Ok(AudioInput {
            original: input.to_string(),
            source_path: dst,
            is_url: true,
            submission_url: Some(input.to_string()),
        });
    }

    // 本地文件
    let path = PathBuf::from(input);
    if path.exists() {
        let canonical = path.canonicalize().unwrap_or(path);
        return Ok(AudioInput {
            original: input.to_string(),
            source_path: canonical,
            is_url: false,
            submission_url: None,
        });
    }

    Err(anyhow!(
        "无法识别输入 '{}'：既不是 URL，也不是存在的本地文件或目录。",
        input
    ))
}

// ---------------------------------------------------------------------------
// URL 下载
// ---------------------------------------------------------------------------

pub async fn download_url(url: &str, dst: &Path, reporter: &dyn ProgressReporter) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("下载失败: {url}"))?
        .error_for_status()?;

    let total = resp.content_length().unwrap_or(0);
    let key = url.to_string();

    let mut file = tokio::fs::File::create(dst).await?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            let pos = (downloaded as f64 / total as f64 * 100.0) as u64;
            reporter.emit(crate::progress::ProgressEvent::Progress {
                key: key.clone(),
                pos,
                len: 100,
            });
        }
    }
    reporter.log(format!("   ✅ 下载完成: {}", dst.display()));
    Ok(())
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

fn sanitize_filename_from_url(url: &str) -> String {
    let base = url
        .split('/')
        .last()
        .unwrap_or("audio.bin")
        .split('?')
        .next()
        .unwrap_or("audio.bin");

    let decoded = urlencoding_decode(base);

    let clean: String = decoded
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();

    if clean.is_empty() || clean == "_" {
        format!("audio_{}.bin", &Uuid::new_v4().to_string()[..8])
    } else {
        clean
    }
}

/// 简易 URL 解码（处理 %20 等常见情况）
fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}
