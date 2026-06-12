//! 输入处理：收集、解析、下载音频输入

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

use crate::types::AudioInput;

// ---------------------------------------------------------------------------
// 收集输入（CLI 参数或交互模式）
// ---------------------------------------------------------------------------

pub async fn gather_inputs(cli_inputs: Option<Vec<String>>) -> Result<Vec<String>> {
    if let Some(ref inputs) = cli_inputs {
        if !inputs.is_empty() {
            let mut expanded = Vec::new();
            for inp in inputs {
                let trimmed = inp.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // 如果是文本文件，展开其内容
                let p = PathBuf::from(trimmed);
                if p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("list"))
                    .unwrap_or(false)
                    && p.exists()
                {
                    let content = fs::read_to_string(&p)
                        .with_context(|| format!("无法读取输入列表文件: {}", p.display()))?;
                    for line in content.lines() {
                        let l = line.trim();
                        if !l.is_empty() && !l.starts_with('#') {
                            expanded.push(l.to_string());
                        }
                    }
                    println!("从 {} 展开 {} 个输入", p.display(), expanded.len());
                } else {
                    expanded.push(trimmed.to_string());
                }
            }
            if expanded.is_empty() {
                return Err(anyhow!("--inputs 参数为空"));
            }
            println!("从命令行参数读取到 {} 个输入", expanded.len());
            return Ok(expanded);
        }
    }

    // 交互模式
    println!("┌─────────────────────────────────────────────┐");
    println!("│  请输入音频输入（每行一个），直接回车结束。 │");
    println!("│  支持：本地文件路径 / HTTP(S) URL / 目录路径  │");
    println!("└─────────────────────────────────────────────┘");

    let mut inputs: Vec<String> = Vec::new();
    let stdin = std::io::stdin();
    let mut first_line = String::new();
    stdin.lock().read_line(&mut first_line)?;
    let first = first_line.trim().to_string();
    if first.is_empty() {
        return Ok(inputs);
    }
    inputs.push(first);

    loop {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            break;
        }
        inputs.push(trimmed);
    }

    // 如果只有一行且是文本文件，展开其内容
    if inputs.len() == 1 {
        let p = PathBuf::from(&inputs[0]);
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("txt") || e.eq_ignore_ascii_case("list"))
            .unwrap_or(false)
            && p.exists()
        {
            let expanded = fs::read_to_string(&p)
                .with_context(|| format!("无法读取输入列表文件: {}", p.display()))?;
            inputs = expanded
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            println!("从 {} 展开 {} 个输入", p.display(), inputs.len());
        }
    }

    Ok(inputs)
}

// ---------------------------------------------------------------------------
// 输入解析：URL → 下载；本地路径 → 直接使用
// ---------------------------------------------------------------------------

pub async fn resolve_input(input: &str, out_dir: &Path) -> Result<AudioInput> {
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
                println!("   ⏭️  文件已存在，跳过下载: {}", dst.display());
                return Ok(AudioInput {
                    original: input.to_string(),
                    source_path: dst,
                    is_url: true,
                    submission_url: Some(input.to_string()),
                });
            }
        }

        println!("   ⬇️  正在下载: {input}");
        download_url(input, &dst).await?;
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

    // 尝试作为目录
    let dir = PathBuf::from(input);
    if !dir.exists() {
        return Err(anyhow!(
            "无法识别输入 '{}'：既不是 URL，也不是存在的本地文件或目录。",
            input
        ));
    }

    Err(anyhow!("输入 '{}' 是一个目录，请直接指定其中的音频文件路径。", input))
}

// ---------------------------------------------------------------------------
// URL 下载
// ---------------------------------------------------------------------------

pub async fn download_url(url: &str, dst: &Path) -> Result<()> {
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
    let pb = if total > 0 {
        ProgressBar::new(total)
    } else {
        ProgressBar::new_spinner()
    };
    pb.set_style(
        ProgressStyle::with_template("   [{elapsed_precise}] {wide_bar} {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );

    let mut file = tokio::fs::File::create(dst).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        pb.inc(chunk.len() as u64);
    }
    pb.finish_and_clear();
    println!("   ✅ 下载完成: {}", dst.display());
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
