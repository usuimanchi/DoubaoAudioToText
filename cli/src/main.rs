//! 多提供商语音转文本批量客户端 — CLI 入口
//!
//! 核心逻辑在 `volc_core` lib，本文件仅负责 CLI 参数解析、交互式配置
//! 和进度输出（通过 `CliProgressReporter` 适配到 stdout）。

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use dialoguer::{Confirm, Input, MultiSelect, Select, theme::ColorfulTheme};
use std::fs;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use volc_core::{input, output, progress::CliProgressReporter, types::*};

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
    #[arg(long, default_value = "ark", help = "提供商: ark | las | volcengine | azure")]
    provider: String,

    // ---- 火山引擎认证 ----
    #[arg(long, help = "火山引擎 X-Api-Key")]
    api_key: Option<String>,
    #[arg(long, help = "火山引擎 Resource ID")]
    resource_id: Option<String>,
    #[arg(long, default_value_t = false, help = "火山引擎旧版控制台")]
    legacy_mode: bool,
    #[arg(long, help = "旧版控制台 App Key")]
    app_key: Option<String>,
    #[arg(long, help = "旧版控制台 Access Key")]
    access_key: Option<String>,

    // ---- Azure 认证 ----
    #[arg(long, help = "Azure subscription key")]
    azure_key: Option<String>,
    #[arg(long, help = "Azure 区域")]
    azure_region: Option<String>,

    // ---- 通用识别选项 ----
    #[arg(long, num_args = 1.., help = "音频文件 URL 或本地路径")]
    inputs: Option<Vec<String>>,
    #[arg(long, help = "主识别语言，如 zh-CN, en-US, ja-JP")]
    language: Option<String>,
    #[arg(long, default_value_t = true, help = "开启说话人分离")]
    enable_speaker_info: bool,
    #[arg(long, default_value_t = true, help = "开启 ITN")]
    enable_itn: bool,
    #[arg(long, default_value_t = true, help = "开启标点")]
    enable_punc: bool,
    #[arg(long, default_value_t = true, help = "开启 DDC 语义顺滑")]
    enable_ddc: bool,
    #[arg(long, default_value_t = true, help = "开启自动语种识别")]
    enable_auto_lang: bool,
    #[arg(long, default_value_t = false, help = "输出分句/分词信息")]
    show_utterances: bool,

    // ---- 火山引擎专用 ----
    #[arg(long, help = "火山引擎: 强制判停时间 300-5000ms")]
    end_window_size: Option<u32>,
    #[arg(long, help = "火山引擎: 自学习热词词表名称")]
    boosting_table_name: Option<String>,
    #[arg(long, help = "火山引擎: 自学习替换词词表名称")]
    correct_table_name: Option<String>,
    #[arg(long, help = "火山引擎: 上下文 JSON 字符串")]
    corpus_context: Option<String>,

    // ---- LAS 算子专用 ----
    #[arg(long, default_value = "doubao-seed-2-0-lite-260428", help = "Ark: 模型名称")]
    ark_model: String,
    #[arg(long, default_value = "cn-beijing", help = "LAS: 服务区域")]
    las_region: String,
    #[arg(long, default_value = "v2", help = "LAS: v2(Seed-ASR 2.0) 失败自动回退v1")]
    operator_version: String,
    #[arg(long, help = "LAS: bigasr 模型版本 310|400")]
    model_version: Option<String>,
    #[arg(long, default_value_t = true, help = "LAS: 开启降噪")]
    enable_denoise: bool,
    #[arg(long, default_value_t = true, help = "LAS: 开启多语种识别")]
    enable_multi_language: bool,
    #[arg(long, default_value_t = false, help = "LAS: 双声道分离识别")]
    enable_channel_split: bool,
    #[arg(long, default_value_t = false, help = "LAS: 分句携带语速")]
    show_speech_rate: bool,
    #[arg(long, default_value_t = false, help = "LAS: 分句携带音量")]
    show_volume: bool,
    #[arg(long, default_value_t = false, help = "LAS: 开启语种识别")]
    enable_lid: bool,
    #[arg(long, default_value_t = false, help = "LAS: 开启情绪检测")]
    enable_emotion_detection: bool,
    #[arg(long, default_value_t = false, help = "LAS: 开启性别检测")]
    enable_gender_detection: bool,
    #[arg(long, help = "LAS: 敏感词过滤")]
    sensitive_words_filter: Option<String>,
    #[arg(long, default_value_t = false, help = "LAS: 开启 POI 地图识别")]
    enable_poi_fc: bool,
    #[arg(long, default_value_t = false, help = "LAS: 开启音乐识别")]
    enable_music_fc: bool,

    // ---- Azure 专用 ----
    #[arg(long, help = "Azure 候选语言 (逗号分隔，最多10个)")]
    candidate_locales: Option<String>,
    #[arg(long, default_value_t = false, help = "Azure: 启用词级时间戳")]
    word_level_timestamps: bool,
    #[arg(long, default_value = "Masked", help = "Azure: 脏话过滤")]
    profanity_filter_mode: String,
    #[arg(long, default_value = "DictatedAndAutomatic", help = "Azure: 标点模式")]
    punctuation_mode: String,

    // ---- 音频处理 ----
    #[arg(long, default_value = DEFAULT_OUTPUT_DIR, help = "输出目录")]
    output_dir: PathBuf,
    #[arg(long, default_value_t = DEFAULT_POLL_INTERVAL_SECS, help = "轮询间隔（秒）")]
    poll_interval_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_DURATION_SECS, help = "单片最大时长（秒）")]
    max_duration_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_SIZE_BYTES, help = "单片最大大小（字节）")]
    max_size_bytes: u64,
    #[arg(long, default_value_t = false, help = "仅准备音频，不提交任务")]
    prepare_only: bool,
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

    // 构建 reporter（CLI: 打印到 stdout）
    let reporter = Arc::new(CliProgressReporter::new());

    // 额外 ffmpeg 搜索目录（exe 同目录，对应 dist 的 ffmpeg.exe）
    let extra_bin_dirs: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .into_iter()
        .collect();

    let mut config = build_config(cli, reporter.clone(), extra_bin_dirs).await?;

    // 确保输出目录存在
    fs::create_dir_all(&config.output_dir)?;
    fs::create_dir_all(config.output_dir.join("prepared"))?;
    fs::create_dir_all(config.output_dir.join("download"))?;
    fs::create_dir_all(config.output_dir.join("results"))?;

    // 持久化 API Key
    let key_hint_path = config.output_dir.join(".last_api_key");
    output::persist_api_key_hint(&key_hint_path, &config.api_key)?;

    // 收集输入
    let is_interactive = cli_inputs.is_none();
    let raw_inputs = if is_interactive {
        gather_inputs_interactive()?
    } else {
        cli_inputs.unwrap_or_default()
    };
    let inputs = input::expand_input_list(raw_inputs, reporter.as_ref())?;
    if inputs.is_empty() {
        return Err(anyhow!(
            "未提供任何音频输入。请通过 --inputs 传参或在交互模式中输入。"
        ));
    }

    // HTTP 客户端
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .context("创建 HTTP 客户端失败")?;

    // 调用核心编排
    volc_core::pipeline::run_pipeline_for_provider(&client, &mut config, &inputs).await
}

// ===========================================================================
// 交互模式输入收集
// ===========================================================================

fn gather_inputs_interactive() -> Result<Vec<String>> {
    println!("┌─────────────────────────────────────────────┐");
    println!("│  请输入音频输入（每行一个），直接回车结束。 │");
    println!("│  支持：本地文件路径 / HTTP(S) URL            │");
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

    Ok(inputs)
}

// ===========================================================================
// Banner
// ===========================================================================

fn print_banner() {
    println!("v{}  ·  doubao-seed-2-0-lite (火山方舟豆包) | --help", env!("CARGO_PKG_VERSION"));
    let lang = volc_core::detect_system_lang();
    let banner = match lang {
        "fr" => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — Transcription Audio              ║
║       Modèle par défaut : doubao-seed-2-0-lite (火山方舟豆包)   ║
╠══════════════════════════════════════════════════════════════════╣
║ Paramètres principaux :                                         ║
║   --api-key <KEY>        Clé API (obligatoire)                 ║
║   --inputs <FICHIERS>    Fichier(s) audio ou URL               ║
║   --provider <NOM>       ark | las | volcengine | azure        ║
║   --language <CODE>      Langue (ex: fr-FR, zh-CN, défaut auto)║
║   --prepare-only         Vérifier sans soumettre               ║
║   --output-dir <DOSSIER> Sortie (répertoire source si local)   ║
╠══════════════════════════════════════════════════════════════════╣
║ Exemple :                                                        ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "https://exemple.com/audio.m4a"                    ║
╠══════════════════════════════════════════════════════════════════╣
║ --help pour la liste complète des paramètres                    ║
╚══════════════════════════════════════════════════════════════════╝
"#,
        "en" => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — Audio Transcription              ║
║       Default model: doubao-seed-2-0-lite (火山方舟豆包)        ║
╠══════════════════════════════════════════════════════════════════╣
║ Main parameters:                                                ║
║   --api-key <KEY>        API Key (required)                    ║
║   --inputs <FILES>       Audio file(s) or URL                  ║
║   --provider <NAME>      ark | las | volcengine | azure        ║
║   --language <CODE>      Language (auto if unset)              ║
║   --prepare-only         Check/convert without submitting       ║
║   --output-dir <DIR>     Output directory                      ║
╠══════════════════════════════════════════════════════════════════╣
║ Example:                                                        ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "https://example.com/audio.m4a"                    ║
╠══════════════════════════════════════════════════════════════════╣
║ --help for full parameter list                                  ║
╚══════════════════════════════════════════════════════════════════╝
"#,
        _ => r#"
╔══════════════════════════════════════════════════════════════════╗
║       Volc AUC Batch Client — 语音转文本批量客户端             ║
║       默认模型: doubao-seed-2-0-lite (火山方舟豆包)              ║
╠══════════════════════════════════════════════════════════════════╣
║ 主要参数:                                                       ║
║   --api-key <KEY>        API Key (必填)                        ║
║   --inputs <FILES>       音频文件路径或 URL (可多个)            ║
║   --provider <NAME>      ark | las | volcengine | azure        ║
║   --language <CODE>      识别语言 (如 fr-FR, zh-CN, 默认 auto) ║
║   --prepare-only         仅检查转换, 不提交                     ║
║   --output-dir <DIR>     输出目录 (本地文件默认音频同目录输出)   ║
╠══════════════════════════════════════════════════════════════════╣
║ 示例:                                                           ║
║   volc_auc_batch_client --api-key "ark-..." \                   ║
║     --inputs "E:\我的音频\示例音频.m4a"                          ║
╠══════════════════════════════════════════════════════════════════╣
║ --help 查看完整参数列表                                         ║
╚══════════════════════════════════════════════════════════════════╝
"#,
    };
    println!("{banner}");
}

// ===========================================================================
// 配置构建
// ===========================================================================

async fn build_config(
    cli: Cli,
    reporter: Arc<CliProgressReporter>,
    extra_bin_dirs: Vec<PathBuf>,
) -> Result<Config> {
    let theme = ColorfulTheme::default();
    let is_interactive = cli.inputs.is_none();

    // === 第 1 步：选择提供商 ===
    let provider = if is_interactive {
        let providers = &[
            "火山方舟豆包 (doubao-seed-2-0-lite) ⭐推荐",
            "火山引擎 AI数据湖服务 (las_asr_pro)",
            "火山方舟录音文件识别服务 (bigmodel)",
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
            let label = if provider == Provider::Ark {
                "Ark API Key"
            } else {
                "LAS API Key"
            };
            let key = match cli.api_key {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => {
                    if let Some(s) = stored_key.filter(|s| !s.is_empty()) {
                        if Confirm::with_theme(&theme)
                            .with_prompt(format!(
                                "使用上次的 API Key（{}...）？",
                                &s[..s.len().min(8)]
                            ))
                            .default(true)
                            .interact()
                            .unwrap_or(true)
                        {
                            s
                        } else {
                            Input::<String>::with_theme(&theme)
                                .with_prompt(format!("请输入 {label}"))
                                .interact_text()?
                        }
                    } else {
                        Input::<String>::with_theme(&theme)
                            .with_prompt(format!("请输入 {label}"))
                            .interact_text()?
                    }
                }
            };
            (key, None, None)
        } else {
            let key = match cli.api_key {
                Some(ref v) if !v.trim().is_empty() => v.clone(),
                _ => {
                    if let Some(s) = stored_key.filter(|s| !s.is_empty()) {
                        if Confirm::with_theme(&theme)
                            .with_prompt(format!(
                                "使用上次的 X-Api-Key（{}...）？",
                                &s[..s.len().min(8)]
                            ))
                            .default(true)
                            .interact()
                            .unwrap_or(true)
                        {
                            s
                        } else {
                            Input::<String>::with_theme(&theme)
                                .with_prompt("请输入 X-Api-Key")
                                .interact_text()?
                        }
                    } else {
                        Input::<String>::with_theme(&theme)
                            .with_prompt("请输入 X-Api-Key")
                            .interact_text()?
                    }
                }
            };
            (key, None, None)
        };

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
        Some(ref s) if !s.trim().is_empty() => Some(
            s.split(',')
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
        ),
        _ => None,
    };

    // --- legacy 校验 ---
    if cli.legacy_mode && (cli.app_key.is_none() || cli.access_key.is_none()) {
        return Err(anyhow!(
            "legacy-mode 需要同时提供 --app-key 和 --access-key"
        ));
    }

    // --- Azure 必填项校验 ---
    if provider == Provider::Azure && azure_key.is_none() {
        return Err(anyhow!("Azure 提供商需要 --azure-key"));
    }
    if provider == Provider::Azure && azure_region.is_none() {
        return Err(anyhow!("Azure 提供商需要 --azure-region"));
    }

    let (max_duration_secs, max_size_bytes) = match provider {
        Provider::Ark => (7170, 512 * 1024 * 1024),
        Provider::Las => (u64::MAX, u64::MAX),
        _ => (7170, 25 * 1024 * 1024),
    };

    Ok(Config {
        provider,
        api_key,
        resource_id,
        legacy_mode: cli.legacy_mode,
        app_key: cli.app_key,
        access_key: cli.access_key,
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
        target_audio_format: if provider == Provider::Ark {
            "mp3".into()
        } else {
            "ogg".into()
        },
        reporter: reporter as Arc<dyn volc_core::ProgressReporter + Send + Sync>,
        extra_bin_dirs,
    })
}
