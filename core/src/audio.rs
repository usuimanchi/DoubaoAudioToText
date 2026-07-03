//! 音频工具：探测、转换、切分（基于 ffmpeg/ffprobe）

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::progress::ProgressReporter;
use crate::types::{Config, PreparedChunk, ProbeMeta, SUPPORTED_FORMATS};

// ---------------------------------------------------------------------------
// ffmpeg/ffprobe 路径解析（跨平台）
// ---------------------------------------------------------------------------

/// 解析 ffmpeg 可执行文件路径。优先 `extra_dirs`（CLI: exe 同目录；Tauri: resource_dir），退回 PATH。
pub fn resolve_ffmpeg(extra_dirs: &[PathBuf]) -> PathBuf {
    resolve_tool("ffmpeg", extra_dirs)
}

/// 解析 ffprobe 可执行文件路径。
pub fn resolve_ffprobe(extra_dirs: &[PathBuf]) -> PathBuf {
    resolve_tool("ffprobe", extra_dirs)
}

fn resolve_tool(name: &str, extra_dirs: &[PathBuf]) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    for d in extra_dirs {
        let p = d.join(&exe);
        if p.exists() {
            return p;
        }
    }
    // 退回 PATH
    PathBuf::from(name)
}

// ---------------------------------------------------------------------------
// ffmpeg 自动检测与下载
// ---------------------------------------------------------------------------

/// 检测 ffmpeg/ffprobe 是否存在，缺失时自动下载到 `target_dir`。
pub async fn ensure_ffmpeg(
    target_dir: &Path,
    extra_dirs: &[PathBuf],
    reporter: &dyn ProgressReporter,
) -> Result<(PathBuf, PathBuf)> {
    let ffmpeg_found = resolve_tool("ffmpeg", extra_dirs).exists() || which("ffmpeg").is_some();
    let ffprobe_found = resolve_tool("ffprobe", extra_dirs).exists() || which("ffprobe").is_some();
    if ffmpeg_found && ffprobe_found {
        return Ok((resolve_ffmpeg(extra_dirs), resolve_ffprobe(extra_dirs)));
    }

    reporter.log("🔍 未检测到 ffmpeg，准备自动下载...".to_string());

    // 检测是否在中国（国内用户走镜像源）
    let in_china = is_likely_china().await;
    if in_china {
        reporter.log("🇨🇳 检测到国内网络，使用国内镜像".to_string());
    } else {
        reporter.log("🌐 使用国际源下载".to_string());
    }

    fs::create_dir_all(target_dir)?;
    download_and_extract_ffmpeg(target_dir, in_china, reporter).await?;

    let ffmpeg_path = resolve_ffmpeg(&[target_dir.to_path_buf()]);
    let ffprobe_path = resolve_ffprobe(&[target_dir.to_path_buf()]);
    if !ffmpeg_path.exists() || !ffprobe_path.exists() {
        return Err(anyhow!("下载 ffmpeg 失败，请手动放入: {}", target_dir.display()));
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&ffmpeg_path, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&ffprobe_path, fs::Permissions::from_mode(0o755))?;
    }
    reporter.log(format!("✅ ffmpeg 就绪: {}", ffmpeg_path.display()));
    Ok((ffmpeg_path, ffprobe_path))
}

/// 快速判断 IP 是否在中国（超时 3s，失败则返回 false）
async fn is_likely_china() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get("https://myip.ipip.net").send().await {
        Ok(resp) => {
            if let Ok(body) = resp.text().await {
                return body.contains("中国");
            }
            false
        }
        Err(_) => false,
    }
}

/// 下载并解压 ffmpeg
async fn download_and_extract_ffmpeg(
    target_dir: &Path,
    in_china: bool,
    reporter: &dyn ProgressReporter,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("创建 HTTP 客户端失败")?;

    #[cfg(windows)]
    {
        let base_url = if in_china {
            "https://mirrors.tuna.tsinghua.edu.cn/ffmpeg/windows/ffmpeg-release-essentials.zip"
        } else {
            "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
        };
        let zip_path = target_dir.join("ffmpeg.zip");
        download_file(&client, base_url, &zip_path, reporter).await?;

        // 用 PowerShell 解压
        let extract_dir = target_dir.join("extract");
        fs::create_dir_all(&extract_dir)?;
        let status = Command::new("powershell")
            .args(["-Command", &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                zip_path.display(), extract_dir.display()
            )])
            .status()?;
        if !status.success() {
            return Err(anyhow!("解压 ffmpeg.zip 失败"));
        }
        // 拷贝 ffmpeg.exe / ffprobe.exe
        for f in &["ffmpeg.exe", "ffprobe.exe"] {
            let dst = target_dir.join(f);
            if !dst.exists() {
                // 递归查找
                find_file(&extract_dir, f, target_dir)?;
            }
        }
        fs::remove_file(&zip_path).ok();
        fs::remove_dir_all(&extract_dir).ok();
    }

    #[cfg(target_os = "macos")]
    {
        let zip_url = if in_china {
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-macos-universal.zip"
        } else {
            "https://evermeet.cx/ffmpeg/ffmpeg-7.1.zip"
        };
        let probe_zip_url = if in_china {
            &zip_url
        } else {
            "https://evermeet.cx/ffmpeg/ffprobe-7.1.zip"
        };
        let zip_path = target_dir.join("ffmpeg.zip");
        download_file(&client, zip_url, &zip_path, reporter).await?;
        let status = Command::new("unzip")
            .args(["-o", &zip_path.to_string_lossy(), "-d", &target_dir.to_string_lossy()])
            .status()?;
        if !status.success() {
            return Err(anyhow!("解压 ffmpeg.zip 失败"));
        }
        fs::remove_file(&zip_path).ok();
        // BtbN 格式：子目录/bin/ffmpeg → 复制到根
        for f in &["ffmpeg", "ffprobe"] {
            if !target_dir.join(f).exists() {
                if let Some(src) = find_file_recursive(target_dir, f) {
                    fs::copy(&src, target_dir.join(f))?;
                }
            }
        }
    }

    reporter.log("✅ ffmpeg 下载并安装完成".to_string());
    Ok(())
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dst: &Path,
    reporter: &dyn ProgressReporter,
) -> Result<()> {
    let resp = client.get(url).send().await
        .with_context(|| format!("下载失败: {url}"))?
        .error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dst).await?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            reporter.emit(crate::progress::ProgressEvent::Progress {
                key: url.to_string(),
                pos: downloaded,
                len: total,
            });
        }
    }
    file.flush().await?;
    Ok(())
}

/// 在 PATH 中查找可执行文件
fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(&exe);
            if full.exists() { Some(full) } else { None }
        })
    })
}

/// 在目录中递归查找文件并复制
fn find_file(src_dir: &Path, name: &str, dst_dir: &Path) -> Result<()> {
    if let Some(src) = find_file_recursive(src_dir, name) {
        fs::copy(&src, dst_dir.join(name))?;
        return Ok(());
    }
    Err(anyhow!("未找到 {name}"))
}

fn find_file_recursive(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, name) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// ffprobe 探测
// ---------------------------------------------------------------------------

pub async fn probe_audio(path: &Path, extra_bin_dirs: &[PathBuf]) -> Result<ProbeMeta> {
    let ffprobe_path = resolve_ffprobe(extra_bin_dirs);
    let output = Command::new(ffprobe_path)
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
    let meta = probe_audio(&input.source_path, &cfg.extra_bin_dirs).await?;

    cfg.reporter.log(format!(
        "   🔍 探测结果: 格式={}  编码={}  {}Hz  {}ch  {}bit  {}  {}",
        meta.format_name,
        meta.codec_name,
        meta.sample_rate,
        meta.channels,
        meta.bits_per_sample,
        format_duration(meta.duration_secs),
        format_size(meta.size_bytes),
    ));

    let format_ok = is_supported_format(&meta.format_name);
    let size_ok = meta.size_bytes <= cfg.max_size_bytes;
    let duration_ok = meta.duration_secs <= cfg.max_duration_secs as f64;

    if format_ok && size_ok && duration_ok {
        cfg.reporter.log("   ✅ 音频符合 API 要求，无需处理。".to_string());

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
        cfg.reporter.warn(format!(
            "   ⚠️  格式不支持: {}（支持: {:?}）",
            meta.format_name,
            SUPPORTED_FORMATS
        ));
        let ext = &cfg.target_audio_format;
        cfg.reporter.log(format!("   🔄 正在转换为 {}...", ext.to_uppercase()));
        let dst = cfg
            .output_dir
            .join("prepared")
            .join(format!("{}_converted.{}", file_stem(&input.source_path), ext));
        if ext == "mp3" {
            convert_to_mp3(&input.source_path, &dst, &meta, &cfg.extra_bin_dirs).await?;
        } else {
            convert_to_ogg(&input.source_path, &dst, &meta, &cfg.extra_bin_dirs).await?;
        }

        let converted_meta = probe_audio(&dst, &cfg.extra_bin_dirs).await?;
        let fmt = ext.to_string();
        let codec = if ext == "mp3" { "mp3" } else { "opus" };
        cfg.reporter.log(format!(
            "   ✅ 转换完成: {}  {}  {}",
            format_duration(converted_meta.duration_secs),
            format_size(converted_meta.size_bytes),
            dst.display()
        ));

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
        cfg.reporter.warn(format!(
            "   ⚠️  时长超限: {}（最大 {} 秒）",
            meta.duration_secs, cfg.max_duration_secs
        ));
    }
    if !size_ok {
        cfg.reporter.warn(format!(
            "   ⚠️  文件大小超限: {}（最大 {}）",
            format_size(meta.size_bytes),
            format_size(cfg.max_size_bytes)
        ));
    }

    if !duration_ok || !size_ok {
        cfg.reporter.log("   🔪 正在切分音频...".to_string());
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

pub async fn convert_to_ogg(src: &Path, dst: &Path, meta: &ProbeMeta, extra_bin_dirs: &[PathBuf]) -> Result<()> {
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

    let ffmpeg_path = resolve_ffmpeg(extra_bin_dirs);
    let status = Command::new(ffmpeg_path)
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

    // 递归深度 > 0 时加后缀，避免输出路径与输入路径碰撞
    let depth_suffix = if depth > 0 { format!("_d{depth}") } else { String::new() };
    let base = cfg.output_dir
        .join("prepared")
        .join(format!("{}_split{}", file_stem(src), depth_suffix));
    fs::create_dir_all(&base)?;

    // 按时长 + 体积双维度计算分段时长
    let duration_segment_secs = cfg.max_duration_secs as f64;
    let segment_secs = if meta.size_bytes > cfg.max_size_bytes {
        let bytes_per_sec = meta.size_bytes as f64 / duration;
        // 按目标体积反算最大时长，留 15% 安全余量
        let size_based_secs = (cfg.max_size_bytes as f64 / bytes_per_sec * 0.85).floor();
        size_based_secs.max(60.0).min(duration_segment_secs)
    } else {
        duration_segment_secs
    };
    let overlap_secs = 10.0; // 每段尾部重叠 10 秒，为边界句子提供上下文
    let total_segments = (duration / segment_secs).ceil() as u32;
    cfg.reporter.log(format!(
        "   🔪 切分为最多 {} 段（每段 ≤ {:.0} 秒，重叠 {} 秒）",
        total_segments, segment_secs, overlap_secs
    ));

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

        let ffmpeg_path = resolve_ffmpeg(&cfg.extra_bin_dirs);
        let mut cmd = Command::new(&ffmpeg_path);
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

        let chunk_meta = probe_audio(&out_path, &cfg.extra_bin_dirs).await?;
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
        // Progress reported via reporter
        cfg.reporter.emit(crate::progress::ProgressEvent::Progress {
            key: format!("split-{}", src.display()),
            pos: idx as u64,
            len: total_segments as u64,
        });
    }
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

pub async fn convert_to_mp3(src: &Path, dst: &Path, meta: &ProbeMeta, extra_bin_dirs: &[PathBuf]) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    // MP3 @ 64kbps mono, good balance for speech
    let ffmpeg_path = resolve_ffmpeg(extra_bin_dirs);
    let status = Command::new(ffmpeg_path)
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

