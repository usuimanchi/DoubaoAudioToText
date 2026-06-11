//! 多提供商语音转文本批量客户端
//!
//! ## 支持的提供商
//!
//! - **火山引擎（豆包大模型）** —— 默认，使用 volc.seedasr.auc
//! - **Azure Speech-to-Text** —— 多语言混合识别能力出色
//!
//! ## 使用方式
//!
//! ```bash
//! # 火山引擎（默认）
//! volc_auc_batch_client --api-key <KEY> --inputs "https://example.com/audio.wav"
//!
//! # Azure（多语言识别）
//! volc_auc_batch_client --provider azure \
//!   --azure-key <KEY> --azure-region eastasia \
//!   --candidate-locales "en-US,zh-CN,ja-JP" \
//!   --inputs "https://example.com/mixed-lang.wav"
//!
//! # 仅检查/准备本地音频，不提交
//! volc_auc_batch_client --inputs ./recording.m4a --prepare-only
//! ```
//!
//! ## 外部依赖
//! - `ffmpeg` / `ffprobe`：音频探测、格式转换与切分

mod ark;
mod audio;
mod azure;
mod backend;
mod input;
mod las;
mod output;
mod tos;
mod types;
mod volcengine;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use dialoguer::{Confirm, Input, MultiSelect, Select, theme::ColorfulTheme};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::backend::{JobHandle, TranscriptionBackend};
use crate::types::*;

// ---------------------------------------------------------------------------
// CLI 参数
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "volc_auc_batch_client",
    author,
    version,
    about = "多提供商语音转文本批量客户端 —— 音频检查/转换/切分 + 批量提交与结果轮询",
    long_about = "支持的提供商：火山引擎（豆包大模型，默认）| Azure Speech-to-Text",
)]
struct Cli {
    // ---- 提供商选择 ----
    /// 语音转文本提供商：volcengine（默认）/ las / azure
    #[arg(long, default_value = "ark", help = "提供商: ark | las | volcengine | azure")]
    provider: String,

    // ---- 火山引擎认证 ----
    /// 火山引擎 API Key（新版控制台的 X-Api-Key）
    #[arg(long, help = "火山引擎 X-Api-Key")]
    api_key: Option<String>,

    /// 火山引擎 Resource ID，默认 volc.seedasr.auc
    #[arg(long, help = "火山引擎 Resource ID")]
    resource_id: Option<String>,

    /// 旧版火山引擎控制台兼容模式
    #[arg(long, default_value_t = false, help = "火山引擎旧版控制台")]
    legacy_mode: bool,

    /// 旧版控制台 App Key
    #[arg(long, help = "旧版控制台 App Key")]
    app_key: Option<String>,

    /// 旧版控制台 Access Key
    #[arg(long, help = "旧版控制台 Access Key")]
    access_key: Option<String>,

    // ---- TOS 对象存储（本地文件自动上传）----
    /// TOS Access Key ID（环境变量: TOS_ACCESS_KEY）
    #[arg(long, help = "TOS: Access Key ID")]
    tos_ak: Option<String>,

    /// TOS Secret Access Key（环境变量: TOS_SECRET_KEY）
    #[arg(long, help = "TOS: Secret Access Key")]
    tos_sk: Option<String>,

    /// TOS 存储桶名称
    #[arg(long, default_value = "amamizu-oss", help = "TOS: 存储桶名称")]
    tos_bucket: String,

    /// TOS 访问域名（不含 https://）
    #[arg(long, default_value = "tos-cn-beijing.volces.com", help = "TOS: 域名")]
    tos_endpoint: String,

    /// TOS 区域
    #[arg(long, default_value = "cn-beijing", help = "TOS: 区域")]
    tos_region: String,

    /// TOS 上传路径前缀（不含文件名），如 "las-audio/"
    #[arg(long, help = "TOS: 上传路径前缀")]
    tos_key_prefix: Option<String>,

    // ---- Azure 认证 ----
    /// Azure Speech 资源的订阅密钥（Ocp-Apim-Subscription-Key）
    #[arg(long, help = "Azure subscription key")]
    azure_key: Option<String>,

    /// Azure 区域（如 eastasia, westeurope, eastus）
    #[arg(long, help = "Azure 区域")]
    azure_region: Option<String>,

    // ---- 通用识别选项 ----
    /// 音频输入：本地文件路径 或 HTTP(S) URL，可传入多个
    #[arg(long, num_args = 1.., help = "音频文件 URL 或本地路径")]
    inputs: Option<Vec<String>>,

    /// 识别语言（逗号分隔）。留空则自动识别。
    #[arg(long, help = "主识别语言，如 zh-CN, en-US, ja-JP")]
    language: Option<String>,

    /// 是否开启说话人分离（Azure: diarization）
    #[arg(long, default_value_t = true, help = "开启说话人分离")]
    enable_speaker_info: bool,

    /// 是否开启文本规范化 ITN（仅火山引擎）
    #[arg(long, default_value_t = true, help = "开启 ITN")]
    enable_itn: bool,

    /// 是否开启标点恢复（仅火山引擎；Azure 通过 punctuation-mode 控制）
    #[arg(long, default_value_t = true, help = "开启标点")]
    enable_punc: bool,

    /// 是否开启语义顺滑 DDC（仅火山引擎）
    #[arg(long, default_value_t = true, help = "开启 DDC 语义顺滑")]
    enable_ddc: bool,

    /// 是否开启自动语种识别（仅火山引擎；Azure 通过 candidate-locales 控制）
    #[arg(long, default_value_t = true, help = "开启自动语种识别")]
    enable_auto_lang: bool,

    /// 是否输出分句信息（仅火山引擎）
    #[arg(long, default_value_t = false, help = "输出分句/分词信息")]
    show_utterances: bool,

    // ---- 火山引擎专用 ----
    /// 强制判停时间（毫秒），范围 300-5000。
    /// 设置后按静音时长分句（VAD），替代默认的语义分句。
    /// 敏感场景可配 500 或更小，普通场景建议 800-1000。
    #[arg(long, help = "火山引擎: 强制判停时间 300-5000ms")]
    end_window_size: Option<u32>,

    /// 自学习平台热词词表名称，参考 https://www.volcengine.com/docs/6561/155738
    #[arg(long, help = "火山引擎: 自学习热词词表名称")]
    boosting_table_name: Option<String>,

    /// 自学习平台替换词词表名称，参考 https://www.volcengine.com/docs/6561/1206007
    #[arg(long, help = "火山引擎: 自学习替换词词表名称")]
    correct_table_name: Option<String>,

    /// 上下文 JSON 字符串。支持三种模式：
    /// 1) 热词直传: '{"hotwords":[{"word":"热词1"},{"word":"热词2"}]}' (最多5000词)
    /// 2) 对话历史: '{"context_type":"dialog_ctx","context_data":[{"text":"..."},{"text":"..."}]}' (最多800 tokens / 20轮)
    /// 3) POI 定位: '{"loc_info":{"city_name":"北京市"}}' (配合 enable_poi_fc)
    /// 豆包2.0 支持在 context_data 中传入 image_url 实现图片理解
    #[arg(long, help = "火山引擎: 上下文 JSON 字符串")]
    corpus_context: Option<String>,

    // ---- LAS 算子专用 ----
    /// Ark 模型名称，默认 doubao-seed-2-0-lite-260428
    #[arg(long, default_value = "doubao-seed-2-0-lite-260428", help = "Ark: 模型名称")]
    ark_model: String,

    /// LAS 服务区域：cn-beijing / cn-shanghai / cn-guangzhou
    #[arg(long, default_value = "cn-beijing", help = "LAS: 服务区域")]
    las_region: String,

    /// LAS 算子版本：v1（默认）或 v2
    #[arg(long, default_value = "v2", help = "LAS: v2(Seed-ASR 2.0,1.6元/h) 失败自动回退v1")]
    operator_version: String,

    /// LAS 模型版本。bigasr: "310"(默认) / "400"(优化版)；seedasr 请勿传此参数
    #[arg(long, help = "LAS: bigasr 模型版本 310|400")]
    model_version: Option<String>,

    /// 是否开启降噪（LAS）
    #[arg(long, default_value_t = true, help = "LAS: 开启降噪")]
    enable_denoise: bool,

    /// 是否开启多语种识别（LAS，默认 true，支持 99 种语言）
    #[arg(long, default_value_t = true, help = "LAS: 开启多语种识别")]
    enable_multi_language: bool,

    /// 是否开启双声道分离（LAS）
    #[arg(long, default_value_t = false, help = "LAS: 双声道分离识别")]
    enable_channel_split: bool,

    /// 分句携带语速（LAS）
    #[arg(long, default_value_t = false, help = "LAS: 分句携带语速")]
    show_speech_rate: bool,

    /// 分句携带音量（LAS）
    #[arg(long, default_value_t = false, help = "LAS: 分句携带音量")]
    show_volume: bool,

    /// 语种识别（LAS：中英/方言）
    #[arg(long, default_value_t = false, help = "LAS: 开启语种识别")]
    enable_lid: bool,

    /// 情绪检测（LAS: angry/happy/neutral/sad/surprise）
    #[arg(long, default_value_t = false, help = "LAS: 开启情绪检测")]
    enable_emotion_detection: bool,

    /// 性别检测（LAS: male/female）
    #[arg(long, default_value_t = false, help = "LAS: 开启性别检测")]
    enable_gender_detection: bool,

    /// 敏感词过滤 JSON 字符串（LAS）
    #[arg(long, help = "LAS: 敏感词过滤")]
    sensitive_words_filter: Option<String>,

    /// POI 地图识别（LAS）
    #[arg(long, default_value_t = false, help = "LAS: 开启 POI 地图识别")]
    enable_poi_fc: bool,

    /// 音乐识别（LAS）
    #[arg(long, default_value_t = false, help = "LAS: 开启音乐识别")]
    enable_music_fc: bool,

    // ---- Azure 专用 ----
    /// 多语言识别的候选语言列表（逗号分隔，最多 10 个）
    /// 例如 "en-US,zh-CN,ja-JP,ko-KR"。设置后启用 Azure Language Identification。
    #[arg(long, help = "Azure 候选语言 (逗号分隔，最多10个)")]
    candidate_locales: Option<String>,

    /// 词级时间戳（Azure）
    #[arg(long, default_value_t = false, help = "Azure: 启用词级时间戳")]
    word_level_timestamps: bool,

    /// 脏话过滤模式（Azure）：None / Masked / Removed / Raw
    #[arg(long, default_value = "Masked", help = "Azure: 脏话过滤")]
    profanity_filter_mode: String,

    /// 标点模式（Azure）：None / Dictated / Automatic / DictatedAndAutomatic
    #[arg(long, default_value = "DictatedAndAutomatic", help = "Azure: 标点模式")]
    punctuation_mode: String,

    // ---- 音频处理 ----
    /// 输出目录
    #[arg(long, default_value = DEFAULT_OUTPUT_DIR, help = "输出目录")]
    output_dir: PathBuf,

    /// 轮询间隔（秒）
    #[arg(long, default_value_t = DEFAULT_POLL_INTERVAL_SECS, help = "轮询间隔（秒）")]
    poll_interval_secs: u64,

    /// 单片最大时长（秒）。Ark 默认 7200s(120min)，LAS 无限制
    #[arg(long, default_value_t = DEFAULT_MAX_DURATION_SECS, help = "单片最大时长（秒，默认 7200 = 120min）")]
    max_duration_secs: u64,

    /// 单片最大大小（字节）。Ark 默认 25MB（URL 方式限制），LAS 无限制
    #[arg(long, default_value_t = DEFAULT_MAX_SIZE_BYTES, help = "单片最大大小（字节，默认 25MB）")]
    max_size_bytes: u64,

    /// 仅检查/准备音频，不提交
    #[arg(long, default_value_t = false, help = "仅准备音频，不提交任务")]
    prepare_only: bool,

    /// 递归切分最大深度
    #[arg(long, default_value_t = DEFAULT_RECURSIVE_DEPTH, help = "递归切分最大深度")]
    max_split_depth: u32,
}

// ===========================================================================
// main
// ===========================================================================

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cli_inputs = cli.inputs.clone();

    // 无参数运行 → 显示帮助并进入交互模式
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        print_banner();
        println!();
    }

    let mut config = build_config(cli).await?;

    // 确保输出目录存在
    fs::create_dir_all(&config.output_dir)?;
    fs::create_dir_all(config.output_dir.join("prepared"))?;
    fs::create_dir_all(config.output_dir.join("download"))?;
    fs::create_dir_all(config.output_dir.join("results"))?;

    // 持久化 API Key
    let key_hint_path = config.output_dir.join(".last_api_key");
    output::persist_api_key_hint(&key_hint_path, &config.api_key)?;

    // 收集输入
    let inputs = input::gather_inputs(cli_inputs).await?;
    if inputs.is_empty() {
        return Err(anyhow!("未提供任何音频输入。请通过 --inputs 传参或在交互模式中输入。"));
    }

    // HTTP 客户端
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .context("创建 HTTP 客户端失败")?;

    // 根据提供商分发
    match config.provider {
        Provider::Volcengine => {
            run_pipeline::<volcengine::VolcengineBackend>(&client, &mut config, &inputs).await
        }
        Provider::Las => {
            run_pipeline::<las::LasBackend>(&client, &mut config, &inputs).await
        }
        Provider::Ark => {
            run_pipeline::<ark::ArkBackend>(&client, &mut config, &inputs).await
        }
        Provider::Azure => {
            run_pipeline::<azure::AzureBackend>(&client, &mut config, &inputs).await
        }
    }
}

// ===========================================================================
// 通用编排流程
// ===========================================================================

async fn run_pipeline<B: TranscriptionBackend>(
    client: &reqwest::Client,
    config: &mut Config,
    inputs: &[String],
) -> Result<()> {
    println!("\n🎙️  提供商: {}", B::provider_name());

    let mut all_summaries: Vec<PersistedSummary> = Vec::new();
    let mut total_submitted = 0usize;
    let mut total_prepared = 0usize;

    for input_str in inputs {
        println!("\n{}", "═".repeat(60));
        println!("📥  处理输入: {input_str}");

        // 0) 输出目录：本地文件 → 源目录；tos:// → 镜像 TOS 路径
        let p = std::path::PathBuf::from(input_str);
        if input_str.starts_with("tos://") && config.output_dir == std::path::PathBuf::from(DEFAULT_OUTPUT_DIR) {
            // tos://bucket/path/to/file.mp3 → ./result/path/to/
            let tos_path = input_str
                .strip_prefix("tos://")
                .and_then(|s| s.splitn(2, '/').nth(1)) // skip bucket name
                .and_then(|s| {
                    let p = std::path::Path::new(s);
                    p.parent().map(|parent| parent.to_path_buf())
                });
            if let Some(tos_dir) = tos_path {
                let sanitized = std::path::PathBuf::from(sanitize_path(&tos_dir.to_string_lossy()));
                config.output_dir = std::path::PathBuf::from(DEFAULT_OUTPUT_DIR).join(&sanitized);
                fs::create_dir_all(&config.output_dir)?;
                fs::create_dir_all(config.output_dir.join("download"))?;
                println!("   📂 输出目录: {}", config.output_dir.display());
            }
        } else if p.exists() && config.output_dir == std::path::PathBuf::from(DEFAULT_OUTPUT_DIR) {
            if let Some(parent) = p.parent() {
                config.output_dir = parent.to_path_buf();
                println!("   📂 输出目录: {}", config.output_dir.display());
            }
        }

        // 1) 解析输入
        let mut audio_input = input::resolve_input(input_str, &config.output_dir).await?;

        // 1.5) tos:// 需要下载才能探测/切分
        if input_str.starts_with("tos://") {
            if let Some(ref tos_ak) = config.tos_ak {
                if !tos_ak.is_empty() {
                    if let Ok(uploader) = tos::create_tos_uploader(
                        tos_ak, config.tos_sk.as_deref().unwrap_or(""),
                        &config.tos_region, &config.tos_endpoint, &config.tos_bucket,
                    ) {
                        let key = input_str.strip_prefix(&format!("tos://{}/", config.tos_bucket)).unwrap_or("");
                        if !key.is_empty() {
                            let dl_path = config.output_dir.join("download").join(sanitize_filename(key));
                            if !dl_path.exists() {
                                println!("   ⬇️  下载 TOS 文件: {key}");
                                match uploader.presigned_url(key, 3600).await {
                                    Ok(ps_url) => {
                                        if let Err(e) = input::download_url(&ps_url, &dl_path).await {
                                            println!("   ⚠️  TOS 下载失败: {e}");
                                        } else {
                                            audio_input.source_path = dl_path;
                                            audio_input.submission_url = None; // 作为本地文件重新处理
                                        }
                                    }
                                    Err(e) => println!("   ⚠️  预签名失败: {e}"),
                                }
                            } else {
                                audio_input.source_path = dl_path;
                            }
                        }
                    }
                }
            }
        }

        // 2) 准备音频（检查/转换/切分）
        let mut prepared_chunks = audio::prepare_audio(&audio_input, config).await?;
        total_prepared += prepared_chunks.len();

        // 2.5) TOS 上传：本地文件自动上传到 TOS 获得 URL
        if let Some(ref tos_ak) = config.tos_ak {
            if !tos_ak.is_empty() {
                let tos_uploader = tos::create_tos_uploader(
                    tos_ak,
                    config.tos_sk.as_deref().unwrap_or(""),
                    &config.tos_region,
                    &config.tos_endpoint,
                    &config.tos_bucket,
                );
                match tos_uploader {
                    Ok(uploader) => {
                        let prefix = config.tos_key_prefix.as_deref().unwrap_or("");
                        for chunk in &mut prepared_chunks {
                            if chunk.submission_url.is_some() {
                                continue; // 已有 URL，跳过
                            }
                            let file_name = chunk.path.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("audio.bin");
                            let remote_key = format!("{prefix}{file_name}");
                            println!("   📤 上传到 TOS: {} → tos://{}/{}", file_name, config.tos_bucket, remote_key);
                            match uploader.upload_file(&chunk.path, &remote_key).await {
                                Ok(result) => {
                                    // 优先使用 tos:// 内网 URL（LAS 和 bigmodel 都支持）
                                    chunk.submission_url = Some(result.tos_url.clone());
                                    println!("   │  ✅ TOS URL: {}", result.tos_url);
                                    // bigmodel 旧 API 需要 HTTPS，生成预签名 URL 备用
                                    if config.provider == Provider::Volcengine || config.provider == Provider::Ark {
                                        match uploader.presigned_url(&remote_key, 86400).await {
                                            Ok(ps_url) => {
                                                chunk.submission_url = Some(ps_url);
                                                println!("   │  🔗 预签名 URL (HTTPS)");
                                            }
                                            Err(e) => println!("   │  ⚠️  预签名失败: {e}"),
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("   │  ⚠️  TOS 上传失败: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        println!("   ⚠️  TOS 客户端初始化失败: {e}");
                    }
                }
            }
        }

        // 输出摘要
        println!("   ┌─ 准备就绪: {} 个片段", prepared_chunks.len());
        for (i, c) in prepared_chunks.iter().enumerate() {
            let dur = audio::format_duration(c.duration_secs);
            let sz = audio::format_size(c.size_bytes);
            let url_info = c.submission_url.as_deref().unwrap_or("（无可提交 URL）");
            println!(
                "   │  [{i}] {dur}  {sz}  格式={} 编码={}  URL={url_info}",
                c.format, c.codec
            );
        }

        // 2.6) bigmodel 旧 API 需要 HTTPS，tos:// → 预签名 URL
        if config.provider == Provider::Volcengine || config.provider == Provider::Ark {
            if let Some(ref tos_ak) = config.tos_ak {
                if !tos_ak.is_empty() {
                    if let Ok(uploader) = tos::create_tos_uploader(
                        tos_ak, config.tos_sk.as_deref().unwrap_or(""),
                        &config.tos_region, &config.tos_endpoint, &config.tos_bucket,
                    ) {
                        for chunk in &mut prepared_chunks {
                            if let Some(ref url) = chunk.submission_url {
                                if url.starts_with("tos://") {
                                    let key = url.strip_prefix(&format!("tos://{}/", config.tos_bucket)).unwrap_or("");
                                    if !key.is_empty() {
                                        println!("   🔗 生成预签名 URL: {key}");
                                        match uploader.presigned_url(key, 86400).await {
                                            Ok(ps) => { println!("   🔗 预签名: {ps}"); chunk.submission_url = Some(ps); }
                                            Err(e) => println!("   ⚠️  预签名失败: {e}"),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3) 筛选可提交片段
        let submittable: Vec<&PreparedChunk> = prepared_chunks
            .iter()
            .filter(|c| c.submission_url.is_some())
            .collect();

        let mut submitted_summaries: Vec<SubmittedTaskSummary> = Vec::new();

        if submittable.is_empty() {
            if audio_input.is_url {
                println!("   ⚠️  该输入为 URL，但音频需要转换/切分。");
                println!("   💡 原始 URL 指向的是未处理的音频，无法用于提交已处理的本地副本。");
                println!("   💡 请将 ./result/prepared/ 下处理好的文件上传到可公网访问的服务器，");
                println!("   💡 然后以新 URL 重新运行本程序。");
            } else {
                println!("   ⚠️  该输入为本地文件，无可提交的 URL，跳过 API 提交。");
                if config.tos_ak.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                    println!("   💡 提示：配置 --tos-ak 和 --tos-sk 可自动上传到 TOS 并获取 URL。");
                }
            }
        } else if config.prepare_only {
            println!(
                "   ⏭️  --prepare-only 模式，跳过 API 提交（共 {} 个可提交片段）。",
                submittable.len()
            );
        } else {
            // 4) 批量提交
            println!("   ┌─ 开始提交 {} 个任务...", submittable.len());
            let mut handles: Vec<JobHandle> = Vec::new();
            for chunk in &submittable {
                match B::submit(client, config, chunk).await {
                    Ok(handle) => {
                        handles.push(handle);
                    }
                    Err(e) => {
                        println!("   │  ❌ 提交失败: {e}");
                    }
                }
            }
            total_submitted += handles.len();

            // 5) 等待完成并保存结果
            for (handle, chunk) in handles.iter().zip(submittable.iter()) {
                match B::wait_for_completion(client, config, handle).await {
                    Ok(output) => {
                        match B::save_result(config, handle, &output, chunk) {
                            Ok(summary) => submitted_summaries.push(summary),
                            Err(e) => {
                                println!("   ❌ 保存结果失败: {e}");
                                submitted_summaries.push(SubmittedTaskSummary {
                                    request_id: handle.id.clone(),
                                    chunk_path: chunk.path.display().to_string(),
                                    submission_url: chunk.submission_url.clone().unwrap_or_default(),
                                    status_code: None,
                                    status_message: Some(format!("{e}")),
                                    result_text: None,
                                    result_json_path: None,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        println!("   ❌ 任务失败: {e}");
                        submitted_summaries.push(SubmittedTaskSummary {
                            request_id: handle.id.clone(),
                            chunk_path: chunk.path.display().to_string(),
                            submission_url: chunk.submission_url.clone().unwrap_or_default(),
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

    // 合并分片结果
    for summary in &all_summaries {
        if summary.submitted.len() > 1 {
            let merged = merge_chunk_results(summary);
            match merged {
                Ok(text) => {
                    let merged_path = config.output_dir.join("result.txt");
                    fs::write(&merged_path, &text)?;
                    println!("   📝 合并文本已保存: {}", merged_path.display());
                    println!("   总字数: {} 字", text.chars().count());
                }
                Err(e) => println!("   ⚠️  合并失败: {e}"),
            }
        }
    }

    // 写入 manifest
    if !all_summaries.is_empty() {
        let manifest_path = config.output_dir.join("manifest.json");
        output::write_manifest(&manifest_path, &all_summaries)?;
    }

    println!(
        "\n🎉 全部完成！准备: {total_prepared} 个片段，提交: {total_submitted} 个任务。"
    );
    Ok(())
}

fn print_banner() {
    println!("{}  ·  doubao-seed-2-0-lite (Ark) | --help", env!("GIT_VERSION"));
    let lang = detect_system_lang();
    let banner = match lang {
        "fr" => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — Transcription Audio              ║
║       Modèle par défaut : doubao-seed-2-0-lite (Ark)           ║
╠══════════════════════════════════════════════════════════════════╣
║ Paramètres principaux :                                         ║
║   --api-key <KEY>        Clé API (obligatoire)                 ║
║   --inputs <FICHIERS>    Fichier(s) audio ou URL               ║
║   --tos-ak <AK>          TOS Access Key                        ║
║   --tos-sk <SK>          TOS Secret Key                        ║
║   --tos-bucket <NOM>     TOS bucket (défaut: amamizu-oss)      ║
║   --provider <NOM>       ark | las | volcengine | azure        ║
║   --language <CODE>      Langue (ex: fr-FR, zh-CN, défaut auto) ║
║   --ark-model <NOM>      Modèle Ark: lite / mini / pro          ║
║   --prepare-only         Vérifier sans soumettre               ║
║   --output-dir <DOSSIER> Sortie (répertoire source si local)    ║
╠══════════════════════════════════════════════════════════════════╣
║ Exemple (URL) :                                                  ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "https://exemple.com/audio.m4a"                    ║
║                                                                ║
║ Exemple (fichier local) :                                       ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --tos-ak "AKLT..." --tos-sk "..." \                         ║
║     --inputs "C:\Audio\exemple.m4a"                              ║
╠══════════════════════════════════════════════════════════════════╣
║ --help pour la liste complète des paramètres                    ║
╚══════════════════════════════════════════════════════════════════╝
"#,
        "en" => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — Audio Transcription              ║
║       Default model: doubao-seed-2-0-lite (Ark)                ║
╠══════════════════════════════════════════════════════════════════╣
║ Main parameters:                                                ║
║   --api-key <KEY>        API Key (required)                    ║
║   --inputs <FILES>       Audio file(s) or URL                  ║
║   --tos-ak <AK>          TOS Access Key                        ║
║   --tos-sk <SK>          TOS Secret Key                        ║
║   --tos-bucket <NAME>    TOS bucket (default: amamizu-oss)      ║
║   --provider <NAME>      ark | las | volcengine | azure        ║
║   --language <CODE>      Language (e.g. fr-FR, zh-CN, default auto)║
║   --ark-model <NAME>     Ark model: lite / mini / pro            ║
║   --prepare-only         Check/convert without submitting       ║
║   --output-dir <DIR>     Output (same as source for local files) ║
╠══════════════════════════════════════════════════════════════════╣
║ Example (URL):                                                  ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "https://example.com/audio.m4a"                    ║
║                                                                ║
║ Example (local file):                                           ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --tos-ak "AKLT..." --tos-sk "..." \                         ║
║     --inputs "C:\Audio\sample.m4a"                               ║
╠══════════════════════════════════════════════════════════════════╣
║ --help for full parameter list                                  ║
╚══════════════════════════════════════════════════════════════════╝
"#,
        _ => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — 语音转文本批量客户端             ║
║       默认模型: doubao-seed-2-0-lite (Ark 方舟)                 ║
╠══════════════════════════════════════════════════════════════════╣
║ 主要参数:                                                       ║
║   --api-key <KEY>        API Key (必填)                        ║
║   --inputs <FILES>       音频文件路径或 URL (可多个)            ║
║   --tos-ak <AK>          TOS Access Key (本地文件上传时填)     ║
║   --tos-sk <SK>          TOS Secret Key                        ║
║   --tos-bucket <NAME>    TOS 桶名 (默认: amamizu-oss)           ║
║   --provider <NAME>      ark | las | volcengine | azure        ║
║   --language <CODE>      识别语言 (如 fr-FR, zh-CN, 默认 auto) ║
║   --ark-model <NAME>     Ark 模型: lite / mini / pro            ║
║   --prepare-only         仅检查转换, 不提交                     ║
║   --output-dir <DIR>     输出目录 (本地文件默认音频同目录输出)   ║
╠══════════════════════════════════════════════════════════════════╣
║ 示例 (网络URL):                                                 ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "https://example.com/audio.m4a"                    ║
║                                                                ║
║ 示例 (本地文件):                                                 ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --tos-ak "AKLT..." --tos-sk "..." \                         ║
║     --inputs "E:\我的音频\示例音频.m4a"                           ║
╠══════════════════════════════════════════════════════════════════╣
║ --help 查看完整参数列表                                         ║
╚══════════════════════════════════════════════════════════════════╝
"#,
    };
    println!("{banner}");
}

fn sanitize_filename(name: &str) -> String {
    // 只替换 Windows/Mac/Linux 文件名和路径中的非法字符，保留中法文字符
    const ILLEGAL: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\0'];
    name.chars()
        .map(|c| if ILLEGAL.contains(&c) || c.is_control() { '_' } else { c })
        .collect()
}

fn sanitize_path(path: &str) -> String {
    // rsplit the path into components, sanitize each, then rejoin
    path.split('/')
        .map(sanitize_filename)
        .collect::<Vec<_>>()
        .join("/")
}

fn detect_system_lang() -> &'static str {
    // Windows: check locale
    if let Ok(locale) = std::env::var("LANG") {
        let l = locale.to_lowercase();
        if l.starts_with("fr") || l.starts_with("fr_") { return "fr"; }
        if l.starts_with("en") || l.starts_with("en_") { return "en"; }
        if l.starts_with("zh") || l.starts_with("zh_") { return "zh"; }
    }
    // Windows: check system locale via powershell-equivalent or just default to zh
    "zh"
}

fn merge_chunk_results(summary: &PersistedSummary) -> Result<String> {
    // 收集所有片段文本
    let texts: Vec<String> = summary.submitted.iter().map(|s| {
        if let Some(ref t) = s.result_text { return t.clone(); }
        if let Some(ref p) = s.result_json_path {
            if let Ok(raw) = std::fs::read_to_string(p) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(t) = crate::ark::extract_text_from_response(&val) { return t; }
                }
            }
        }
        String::new()
    }).collect();

    if texts.is_empty() { return Ok(String::new()); }
    if texts.len() == 1 { return Ok(texts[0].clone()); }

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
/// overlap_len 是 next 中应跳过的字符数。
fn dedup_overlap(prev: &str, next: &str) -> (String, usize) {
    let tail = prev.chars().rev().take(150).collect::<Vec<_>>().into_iter().rev().collect::<String>();
    let head: String = next.chars().take(150).collect();

    if tail.is_empty() || head.is_empty() { return (prev.to_string(), 0); }

    // 找 tail 的每个后缀是否等于 head 的对应前缀（最长匹配）
    let mut best = 0usize;
    for len in (10..=tail.chars().count().min(head.chars().count())).rev() {
        let tail_suffix: String = tail.chars().rev().take(len).collect::<Vec<_>>().into_iter().rev().collect();
        let head_prefix: String = head.chars().take(len).collect();
        if tail_suffix == head_prefix {
            best = len;
            break;
        }
    }

    if best < 15 {
        // 重叠太短，不去重
        return (prev.to_string(), 0);
    }

    let without_overlap: String = prev.chars().take(prev.chars().count() - best).collect();
    (without_overlap, best)
}

// ===========================================================================
// 配置构建
// ===========================================================================

async fn build_config(cli: Cli) -> Result<Config> {
    let theme = ColorfulTheme::default();
    let is_interactive = cli.inputs.is_none();  // 无 --inputs = 交互模式

    // === 第 1 步：选择提供商 ===
    let provider = if is_interactive {
        let providers = &[
            "Ark 方舟 (doubao-seed-2-0-lite) ⭐推荐",
            "LAS 算子 (las_asr_pro)",
            "Volcengine bigmodel (openspeech)",
            "Azure Speech-to-Text",
        ];
        let idx = Select::with_theme(&theme)
            .with_prompt("选择语音识别提供商")
            .items(providers)
            .default(0)
            .interact()?;
        match idx {
            1 => Provider::Las,
            2 => Provider::Volcengine,
            3 => Provider::Azure,
            _ => Provider::Ark,
        }
    } else {
        match cli.provider.as_str() {
            "azure" | "Azure" => Provider::Azure,
            "las" | "LAS" | "las_asr_pro" => Provider::Las,
            "ark" | "Ark" | "ARK" => Provider::Ark,
            "volcengine" | "volc" => Provider::Volcengine,
            "" | _ => Provider::Ark,
        }
    };

    // === 第 2 步：API Key ===
    let stored_key_path = PathBuf::from(DEFAULT_OUTPUT_DIR).join(".last_api_key");
    let stored_key = output::load_last_api_key_hint(&stored_key_path);

    let (api_key, azure_key, azure_region) =
        if provider == Provider::Azure {
            let az_key = match cli.azure_key {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => Input::<String>::with_theme(&theme)
                    .with_prompt("Azure Subscription Key")
                    .interact_text()?,
            };
            let az_region = match cli.azure_region {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => Input::<String>::with_theme(&theme)
                    .with_prompt("Azure Region（如 eastasia）")
                    .default("eastasia".into())
                    .interact_text()?,
            };
            (az_key.clone(), Some(az_key), Some(az_region))
        } else if provider == Provider::Las || provider == Provider::Ark {
            let label = if provider == Provider::Ark { "Ark API Key" } else { "LAS API Key" };
            let key = match cli.api_key {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => {
                    if let Some(s) = stored_key.filter(|s| !s.is_empty()) {
                        if Confirm::with_theme(&theme)
                            .with_prompt(format!("使用上次的 API Key（{}...）？", &s[..s.len().min(8)]))
                            .default(true).interact().unwrap_or(true) { s }
                        else { Input::<String>::with_theme(&theme).with_prompt(format!("请输入 {label}")).interact_text()? }
                    } else { Input::<String>::with_theme(&theme).with_prompt(format!("请输入 {label}")).interact_text()? }
                }
            };
            (key, None, None)
        } else {
            let key = match cli.api_key {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => {
                    if let Some(s) = stored_key.filter(|s| !s.is_empty()) {
                        if Confirm::with_theme(&theme)
                            .with_prompt(format!("使用上次的 X-Api-Key（{}...）？", &s[..s.len().min(8)]))
                            .default(true).interact().unwrap_or(true) { s }
                        else { Input::<String>::with_theme(&theme).with_prompt("请输入 X-Api-Key").interact_text()? }
                    } else { Input::<String>::with_theme(&theme).with_prompt("请输入 X-Api-Key").interact_text()? }
                }
            };
            (key, None, None)
        };

    // === 第 3 步：TOS 配置（Ark/LAS/Volcengine 本地文件上传需要）===
    let (tos_ak, tos_sk, tos_bucket) = if provider != Provider::Azure && is_interactive {
        if cli.tos_ak.is_some() { (cli.tos_ak, cli.tos_sk, cli.tos_bucket) }
        else {
            let ak = Input::<String>::with_theme(&theme)
                .with_prompt("TOS Access Key（本地文件上传需要，回车跳过）")
                .allow_empty(true).interact_text()?;
            if ak.is_empty() { (None, None, cli.tos_bucket) }
            else {
                let sk = Input::<String>::with_theme(&theme)
                    .with_prompt("TOS Secret Key").interact_text()?;
                let bucket = Input::<String>::with_theme(&theme)
                    .with_prompt("TOS 桶名").default("amamizu-oss".into()).interact_text()?;
                (Some(ak), Some(sk), bucket)
            }
        }
    } else { (cli.tos_ak, cli.tos_sk, cli.tos_bucket) };

    // --- Resource ID ---
    let resource_id = cli
        .resource_id
        .unwrap_or_else(|| DEFAULT_RESOURCE_ID.to_string());

    // --- Language ---
    let language = match cli.language {
        Some(ref l) if !l.trim().is_empty() => Some(l.trim().to_string()),
        _ => {
            let langs = vec![
                "（自动识别 / 留空）",
                "zh-CN  中文普通话",
                "en-US  英语",
                "ja-JP  日语",
                "ko-KR  韩语",
                "yue-CN 粤语",
                "de-DE  德语",
                "fr-FR  法语",
                "es-MX  西班牙语",
                "pt-BR  葡萄牙语",
            ];
            let selections = MultiSelect::with_theme(&theme)
                .with_prompt("选择主识别语言（空格选中，回车确认；留空则自动识别）")
                .items(&langs)
                .interact()
                .unwrap_or_default();

            if selections.is_empty() {
                None
            } else {
                let selected: Vec<&str> = selections
                    .iter()
                    .map(|&i| langs[i].split_whitespace().next().unwrap_or(""))
                    .filter(|s| !s.is_empty() && *s != "（自动识别")
                    .collect();
                if selected.is_empty() {
                    None
                } else {
                    Some(selected.join(","))
                }
            }
        }
    };

    // --- 候选语言（Azure 多语言 ID）---
    let candidate_locales: Option<Vec<String>> = match cli.candidate_locales {
        Some(ref s) if !s.trim().is_empty() => {
            Some(s.split(',').map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        }
        _ => None,
    };

    // --- legacy 校验 ---
    if cli.legacy_mode && (cli.app_key.is_none() || cli.access_key.is_none()) {
        return Err(anyhow!("legacy-mode 需要同时提供 --app-key 和 --access-key"));
    }

    // --- Azure 必填项校验 ---
    if provider == Provider::Azure && azure_key.is_none() {
        return Err(anyhow!("Azure 提供商需要 --azure-key"));
    }
    if provider == Provider::Azure && azure_region.is_none() {
        return Err(anyhow!("Azure 提供商需要 --azure-region"));
    }

    // LAS 算子不限大小和时长；Ark 用预设默认值（120min/25MB）
    let (max_duration_secs, max_size_bytes) = if provider == Provider::Las {
        (u64::MAX, u64::MAX)
    } else {
        (cli.max_duration_secs, cli.max_size_bytes)
    };

    Ok(Config {
        provider,
        api_key,
        resource_id,
        legacy_mode: cli.legacy_mode,
        app_key: cli.app_key,
        access_key: cli.access_key,
        tos_ak,
        tos_sk,
        tos_bucket,
        tos_endpoint: cli.tos_endpoint,
        tos_region: cli.tos_region,
        tos_key_prefix: cli.tos_key_prefix,
        azure_key,
        azure_region,
        language,
        enable_speaker_info: cli.enable_speaker_info,
        enable_itn: cli.enable_itn,
        enable_punc: cli.enable_punc,
        enable_ddc: cli.enable_ddc,
        enable_auto_lang: cli.enable_auto_lang,
        show_utterances: cli.show_utterances,
        end_window_size: cli.end_window_size,
        boosting_table_name: cli.boosting_table_name,
        correct_table_name: cli.correct_table_name,
        corpus_context: cli.corpus_context,
        ark_model: cli.ark_model,
        las_region: cli.las_region,
        operator_version: cli.operator_version,
        model_version: cli.model_version,
        enable_denoise: cli.enable_denoise,
        enable_multi_language: cli.enable_multi_language,
        enable_channel_split: cli.enable_channel_split,
        show_speech_rate: cli.show_speech_rate,
        show_volume: cli.show_volume,
        enable_lid: cli.enable_lid,
        enable_emotion_detection: cli.enable_emotion_detection,
        enable_gender_detection: cli.enable_gender_detection,
        sensitive_words_filter: cli.sensitive_words_filter,
        enable_poi_fc: cli.enable_poi_fc,
        enable_music_fc: cli.enable_music_fc,
        candidate_locales,
        word_level_timestamps: cli.word_level_timestamps,
        profanity_filter_mode: cli.profanity_filter_mode,
        punctuation_mode: cli.punctuation_mode,
        output_dir: cli.output_dir,
        poll_interval_secs: cli.poll_interval_secs,
        max_duration_secs,
        max_size_bytes,
        prepare_only: cli.prepare_only,
        max_split_depth: cli.max_split_depth,
        target_audio_format: if provider == Provider::Ark { "mp3".into() } else { "ogg".into() },
    })
}
