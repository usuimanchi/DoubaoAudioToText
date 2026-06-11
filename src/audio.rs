//! 音频工具：探测、转换、切分（基于 ffmpeg/ffprobe）

use anyhow::{anyhow, Context, Result};
use indicatif::ProgressBar;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::types::{Config, PreparedChunk, ProbeMeta, SUPPORTED_FORMATS};

// ---------------------------------------------------------------------------
// ffprobe 探测
// ---------------------------------------------------------------------------

pub async fn probe_audio(path: &Path) -> Result<ProbeMeta> {
    let output = Command::new("ffprobe")
        .arg("-v").arg("error")
        .arg("-show_entries")
        .arg("format=format_name,duration,bit_rate:stream=codec_name,sample_rate,channels,bits_per_raw_sample")
        .arg("-of").arg("default=noprint_wrappers=1:nokey=0")
        .arg(path)
        .output()
        .with_context(|| "执行 ffprobe 失败，请确认已安装 ffmpeg。下载地址: https://ffmpeg.org/download.html")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe 分析失败 ({}): {}", path.display(), stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut format_name = String::from("unknown");
    let mut codec_name = String::from("unknown");
    let mut sample_rate: u32 = 16000;
    let mut bitrate_bps: u64 = 0;
    let mut channels: u32 = 1;
    let mut bits_per_sample: u32 = 16;
    let mut duration_secs: f64 = 0.0;

    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("format_name=") {
            format_name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("duration=") {
            duration_secs = v.trim().parse().unwrap_or(0.0);
        } else if let Some(v) = line.strip_prefix("bit_rate=") {
            bitrate_bps = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("codec_name=") {
            codec_name = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("sample_rate=") {
            sample_rate = v.trim().parse().unwrap_or(16000);
        } else if let Some(v) = line.strip_prefix("channels=") {
            channels = v.trim().parse().unwrap_or(1);
        } else if let Some(v) = line.strip_prefix("bits_per_raw_sample=") {
            bits_per_sample = v.trim().parse().unwrap_or(16);
        }
    }

    let size_bytes = fs::metadata(path)?.len();

    Ok(ProbeMeta {
        format_name,
        codec_name,
        sample_rate,
        bitrate_bps,
        channels,
        bits_per_sample,
        duration_secs,
        size_bytes,
    })
}

// ---------------------------------------------------------------------------
// 音频准备流程
// ---------------------------------------------------------------------------

pub async fn prepare_audio(
    input: &crate::types::AudioInput,
    cfg: &Config,
) -> Result<Vec<PreparedChunk>> {
    // tos:// URL 已上传，跳过本地探测，直接返回
    if let Some(ref url) = input.submission_url {
        if url.starts_with("tos://") {
            println!("   📎 TOS URL，跳过本地探测: {url}");
            // 从扩展名推断格式
            let fmt = url.rsplit('.').next().unwrap_or("ogg");
            let codec = match fmt {
                "wav" => "raw", "mp3" => "mp3", _ => "opus",
            };
            return Ok(vec![PreparedChunk {
                path: input.source_path.clone(),
                format: fmt.to_string(),
                codec: codec.to_string(),
                sample_rate: 16000,
                duration_secs: 0.0,
                size_bytes: 0,
                submission_url: Some(url.clone()),
            }]);
        }
    }

    let meta = probe_audio(&input.source_path).await?;

    println!("   🔍 探测结果: 格式={}  编码={}  {}Hz  {}ch  {}bit  {}  {}",
             meta.format_name,
             meta.codec_name,
             meta.sample_rate,
             meta.channels,
             meta.bits_per_sample,
             format_duration(meta.duration_secs),
             format_size(meta.size_bytes),
    );

    let format_ok = is_supported_format(&meta.format_name);
    let size_ok = meta.size_bytes <= cfg.max_size_bytes;
    let duration_ok = meta.duration_secs <= cfg.max_duration_secs as f64;

    if format_ok && size_ok && duration_ok {
        println!("   ✅ 音频符合 API 要求，无需处理。");

        let (fmt, codec) = normalize_format_and_codec(&meta);
        return Ok(vec![PreparedChunk {
            path: input.source_path.clone(),
            format: fmt,
            codec,
            sample_rate: meta.sample_rate,
            duration_secs: meta.duration_secs,
            size_bytes: meta.size_bytes,
            submission_url: input.submission_url.clone(),
        }]);
    }

    // 报告不合规项并处理
    if !format_ok {
        println!("   ⚠️  格式不支持: {}（支持: {:?}）", meta.format_name, SUPPORTED_FORMATS);
        let ext = &cfg.target_audio_format;
        println!("   🔄 正在转换为 {}...", ext.to_uppercase());
        let dst = cfg.output_dir
            .join("prepared")
            .join(format!("{}_converted.{}", file_stem(&input.source_path), ext));
        if ext == "mp3" {
            convert_to_mp3(&input.source_path, &dst, &meta).await?;
        } else {
            convert_to_ogg(&input.source_path, &dst, &meta).await?;
        }

        let converted_meta = probe_audio(&dst).await?;
        let fmt = ext.to_string();
        let codec = if ext == "mp3" { "mp3" } else { "opus" };
        println!("   ✅ 转换完成: {}  {}  {}",
                 format_duration(converted_meta.duration_secs),
                 format_size(converted_meta.size_bytes),
                 dst.display());

        if converted_meta.size_bytes > cfg.max_size_bytes
            || converted_meta.duration_secs > cfg.max_duration_secs as f64
        {
            println!("   ⚠️  转换后仍超限，继续切分...");
            return split_audio(&dst, cfg, &converted_meta, 0).await;
        }

        return Ok(vec![PreparedChunk {
            path: dst,
            format: ext.to_string(),
            codec: if ext == "mp3" { "mp3".into() } else { "opus".into() },
            sample_rate: converted_meta.sample_rate,
            duration_secs: converted_meta.duration_secs,
            size_bytes: converted_meta.size_bytes,
            submission_url: None,
        }]);
    }

    if !duration_ok {
        println!("   ⚠️  时长超限: {}（最大 {} 秒）", meta.duration_secs, cfg.max_duration_secs);
    }
    if !size_ok {
        println!("   ⚠️  文件大小超限: {}（最大 {}）", format_size(meta.size_bytes), format_size(cfg.max_size_bytes));
    }

    if !duration_ok || !size_ok {
        println!("   🔪 正在切分音频...");
        return split_audio(&input.source_path, cfg, &meta, 0).await;
    }

    let (fmt, codec) = normalize_format_and_codec(&meta);
    Ok(vec![PreparedChunk {
        path: input.source_path.clone(),
        format: fmt,
        codec,
        sample_rate: meta.sample_rate,
        duration_secs: meta.duration_secs,
        size_bytes: meta.size_bytes,
        submission_url: input.submission_url.clone(),
    }])
}

// ---------------------------------------------------------------------------
// 格式检测
// ---------------------------------------------------------------------------

pub fn is_supported_format(format_name: &str) -> bool {
    let f = format_name.to_ascii_lowercase();
    SUPPORTED_FORMATS.iter().any(|s| f.contains(s))
        || f.contains("pcm")
        || f.contains("wav")
}

/// 从 ProbeMeta 返回 (容器格式, 编码格式) 的 API 命名
pub fn normalize_format_and_codec(meta: &ProbeMeta) -> (String, String) {
    let fmt = meta.format_name.to_ascii_lowercase();
    let codec = meta.codec_name.to_ascii_lowercase();

    let container = if fmt.contains("wav") {
        "wav"
    } else if fmt.contains("mp3") {
        "mp3"
    } else if fmt.contains("ogg") || fmt.contains("opus") {
        "ogg"
    } else if fmt.contains("pcm") || fmt.contains("raw") {
        "raw"
    } else {
        "ogg"
    };

    let codec_str = if codec.contains("opus") {
        "opus"
    } else if codec.contains("pcm") {
        "raw"
    } else if codec.contains("mp3") {
        "mp3"
    } else if codec.contains("vorbis") {
        "vorbis"
    } else if codec.contains("aac") {
        "aac"
    } else {
        "raw"
    };

    (container.to_string(), codec_str.to_string())
}

// ---------------------------------------------------------------------------
// 音频转换: → 16bit OGG (Opus)，尽量保持原始质量
// ---------------------------------------------------------------------------

pub async fn convert_to_ogg(src: &Path, dst: &Path, meta: &ProbeMeta) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // Opus 只支持 48000, 24000, 16000, 12000, 8000 Hz
    let out_sample_rate = nearest_opus_rate(meta.sample_rate);

    let per_channel_bitrate = if meta.bitrate_bps > 0 {
        let br = meta.bitrate_bps / meta.channels as u64;
        br.clamp(16000, 128000)
    } else {
        48000
    };

    println!("   🎛️  转换参数: {}Hz mono opus @ {}kbps（原始: {}Hz {}ch {}kbps）",
             out_sample_rate,
             per_channel_bitrate / 1000,
             meta.sample_rate,
             meta.channels,
             meta.bitrate_bps / 1000);

    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i").arg(src)
        .arg("-ac").arg("1")
        .arg("-ar").arg(out_sample_rate.to_string())
        .arg("-c:a").arg("libopus")
        .arg("-b:a").arg(format!("{}", per_channel_bitrate))
        .arg("-vbr").arg("on")
        .arg("-compression_level").arg("10")
        .arg("-application").arg("audio")
        .arg(dst)
        .status()
        .with_context(|| "执行 ffmpeg 转换失败，请确认已安装 ffmpeg")?;

    if !status.success() {
        return Err(anyhow!("转换为 OGG 失败：ffmpeg 返回非零退出码"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 音频切分：按时长切分为多个片段
// ---------------------------------------------------------------------------

pub async fn split_audio(
    src: &Path,
    cfg: &Config,
    meta: &ProbeMeta,
    depth: u32,
) -> Result<Vec<PreparedChunk>> {
    if depth >= cfg.max_split_depth {
        return Err(anyhow!(
            "切分深度已达上限 ({})，文件 {}（{} / {}）仍超限。请增大 --max-duration-secs 或 --max-size-bytes。",
            cfg.max_split_depth,
            src.display(),
            format_duration(meta.duration_secs),
            format_size(meta.size_bytes),
        ));
    }

    let duration = meta.duration_secs;
    if duration <= 0.0 {
        return Err(anyhow!("无法获取音频时长，不能切分: {}", src.display()));
    }

    let base = cfg.output_dir
        .join("prepared")
        .join(format!("{}_split", file_stem(src)));
    fs::create_dir_all(&base)?;

    let segment_secs = cfg.max_duration_secs as f64;
    let overlap_secs = 10.0; // 每段尾部重叠 10 秒，为边界句子提供上下文
    let total_segments = (duration / segment_secs).ceil() as u32;
    println!("   🔪 切分为最多 {} 段（每段 ≤ {} 秒，重叠 {} 秒）", total_segments, segment_secs, overlap_secs);

    let pb = ProgressBar::new(total_segments as u64);
    pb.set_style(
        indicatif::ProgressStyle::with_template("[{elapsed_precise}] {wide_bar} {pos}/{len}")
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar()),
    );

    let mut parts: Vec<PreparedChunk> = Vec::new();
    let mut start = 0.0;
    let mut idx = 0usize;

    let ext = &cfg.target_audio_format;
    let (codec, enc, bitrate): (&str, &str, &str) = if ext == "mp3" {
        ("mp3", "libmp3lame", "64k")
    } else {
        ("opus", "libopus", "48k")
    };

    while start < duration {
        let remaining = duration - start;
        if remaining < 1.0 { break; } // 剩余不足 1 秒，跳过
        let this_duration = (segment_secs + overlap_secs).min(remaining);

        let out_path = base.join(format!("part_{:04}.{ext}", idx));

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-y").arg("-i").arg(src)
            .arg("-ss").arg(format!("{start}"))
            .arg("-t").arg(format!("{this_duration}"))
            .arg("-ac").arg("1")
            .arg("-ar").arg(meta.sample_rate.to_string())
            .arg("-c:a").arg(enc)
            .arg("-b:a").arg(bitrate);
        if ext == "ogg" {
            cmd.arg("-vbr").arg("on").arg("-application").arg("audio");
        }
        let status = cmd.arg(&out_path).status()
            .with_context(|| format!("ffmpeg 切分失败（片段 {}）", idx))?;

        if !status.success() {
            return Err(anyhow!("切分音频失败（片段 {}）", idx));
        }

        let chunk_meta = probe_audio(&out_path).await?;
        if chunk_meta.size_bytes > cfg.max_size_bytes && depth + 1 < cfg.max_split_depth {
            let sub_parts = Box::pin(split_audio(&out_path, cfg, &chunk_meta, depth + 1)).await?;
            parts.extend(sub_parts);
            fs::remove_file(&out_path).ok();
        } else {
            parts.push(PreparedChunk {
                path: out_path,
                format: ext.to_string(),
                codec: codec.to_string(),
                sample_rate: chunk_meta.sample_rate,
                duration_secs: chunk_meta.duration_secs,
                size_bytes: chunk_meta.size_bytes,
                submission_url: None,
            });
        }

        start += segment_secs;
        idx += 1;
        pb.inc(1);
    }

    pb.finish_and_clear();
    Ok(parts)
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

pub fn file_stem(path: &Path) -> String {
    let name = path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio");
    const ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\0'];
    name.chars()
        .map(|c| if ILLEGAL.contains(&c) || c.is_control() { '_' } else { c })
        .collect()
}

pub fn format_duration(secs: f64) -> String {
    let h = (secs / 3600.0) as u64;
    let m = ((secs % 3600.0) / 60.0) as u64;
    let s = (secs % 60.0) as u64;
    if h > 0 {
        format!("{}h{}m{}s", h, m, s)
    } else if m > 0 {
        format!("{}m{}s", m, s)
    } else {
        format!("{:.1}s", secs)
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", size, UNITS[unit_idx])
}

pub async fn convert_to_mp3(src: &Path, dst: &Path, meta: &ProbeMeta) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    // MP3 @ 64kbps mono, good balance for speech
    let status = Command::new("ffmpeg")
        .arg("-y").arg("-i").arg(src)
        .arg("-ac").arg("1")
        .arg("-ar").arg("16000")
        .arg("-c:a").arg("libmp3lame")
        .arg("-b:a").arg("64k")
        .arg(dst)
        .status()
        .with_context(|| "执行 ffmpeg 转换 MP3 失败")?;
    if !status.success() {
        return Err(anyhow!("转换为 MP3 失败：ffmpeg 返回非零退出码"));
    }
    Ok(())
}

/// 找到最接近的 Opus 有效采样率（8000 / 12000 / 16000 / 24000 / 48000）
fn nearest_opus_rate(rate: u32) -> u32 {
    const VALID: &[u32] = &[8000, 12000, 16000, 24000, 48000];
    VALID
        .iter()
        .min_by_key(|&&r| {
            if r > rate { r - rate } else { rate - r }
        })
        .copied()
        .unwrap_or(16000)
}

