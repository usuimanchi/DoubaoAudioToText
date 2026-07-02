//! 共享数据类型和常量

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::progress::ProgressReporter;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

pub const DEFAULT_RESOURCE_ID: &str = "seedasr";
pub const DEFAULT_OUTPUT_DIR: &str = "./result";
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_MAX_DURATION_SECS: u64 = 7170;        // Ark Files API 单文件限制 2h，留 30s 余量
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 512 * 1024 * 1024; // Files API: 512 MB
pub const DEFAULT_RECURSIVE_DEPTH: u32 = 3;

/// 支持的音频容器格式（API 文档：raw / wav / mp3 / ogg）
pub const SUPPORTED_FORMATS: &[&str] = &["wav", "mp3", "ogg", "pcm", "raw"];

/// 支持的音频编码格式
pub const SUPPORTED_CODECS: &[&str] = &[
    "pcm_s16le", "pcm_s16be", "pcm_s24le", "pcm_f32le",
    "opus", "mp3", "aac", "vorbis", "flac",
];

// ---------------------------------------------------------------------------
// 提供商枚举
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "volcengine")]
    Volcengine,
    #[serde(rename = "azure")]
    Azure,
    #[serde(rename = "las")]
    Las,
    #[serde(rename = "ark")]
    Ark,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Volcengine => "volcengine",
            Provider::Azure => "azure",
            Provider::Las => "las",
            Provider::Ark => "ark",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Ark => "火山方舟豆包（volcengine ark doubao）",
            Provider::Las => "火山引擎 AI数据湖服务（volcengine）",
            Provider::Volcengine => "火山方舟录音文件识别服务",
            Provider::Azure => "Azure Speech-to-Text",
        }
    }
}

// ---------------------------------------------------------------------------
// 运行时配置
// ---------------------------------------------------------------------------

pub struct Config {
    // --- 提供商 ---
    pub provider: Provider,

    // --- 火山引擎认证 ---
    pub api_key: String,
    pub resource_id: String,
    pub legacy_mode: bool,
    pub app_key: Option<String>,
    pub access_key: Option<String>,

    // --- Azure 认证 ---
    pub azure_key: Option<String>,
    pub azure_region: Option<String>,

    // --- 通用识别选项 ---
    pub language: Option<String>,
    pub enable_speaker_info: bool,
    pub enable_itn: bool,
    pub enable_punc: bool,
    pub enable_ddc: bool,
    pub enable_auto_lang: bool,
    pub show_utterances: bool,

    // --- 火山引擎专用选项 ---
    /// 强制判停时间（毫秒），范围 300-5000。设置后使用静音分句替代语义分句
    pub end_window_size: Option<u32>,
    /// 自学习平台热词词表名称
    pub boosting_table_name: Option<String>,
    /// 自学习平台替换词词表名称
    pub correct_table_name: Option<String>,
    /// 上下文 JSON 字符串（支持 hotwords 直传、dialog_ctx 对话历史、image_url 图片理解）
    pub corpus_context: Option<String>,

    // --- LAS 算子专用选项 ---
    /// LAS 服务区域（默认 cn-beijing）
    /// Ark 模型名称（默认 doubao-seed-2-0-lite-260428）
    pub ark_model: String,
    pub las_region: String,
    /// 算子版本（v1 / v2）
    pub operator_version: String,
    /// 模型版本。bigasr: "310"(默认) / "400"(优化版)；seedasr 请勿传
    pub model_version: Option<String>,
    /// 是否开启降噪
    pub enable_denoise: bool,
    /// 是否开启多语种识别（默认 true，支持 99 种语言）
    pub enable_multi_language: bool,
    /// 双声道分离识别
    pub enable_channel_split: bool,
    /// 分句携带语速
    pub show_speech_rate: bool,
    /// 分句携带音量
    pub show_volume: bool,
    /// 语种识别（中英/方言）
    pub enable_lid: bool,
    /// 情绪检测（angry/happy/neutral/sad/surprise）
    pub enable_emotion_detection: bool,
    /// 性别检测（male/female）
    pub enable_gender_detection: bool,
    /// 敏感词过滤 JSON 字符串
    pub sensitive_words_filter: Option<String>,
    /// POI 地图 function call
    pub enable_poi_fc: bool,
    /// 音乐 function call
    pub enable_music_fc: bool,

    // --- Azure 专用选项 ---
    /// 多语言识别的候选语言列表（逗号分隔的 locale 字符串）
    pub candidate_locales: Option<Vec<String>>,
    pub word_level_timestamps: bool,
    pub profanity_filter_mode: String,
    pub punctuation_mode: String,

    // --- 音频处理 ---
    pub output_dir: PathBuf,
    pub poll_interval_secs: u64,
    pub max_duration_secs: u64,
    pub max_size_bytes: u64,
    pub prepare_only: bool,
    pub max_split_depth: u32,
    /// 目标转换格式（ogg / mp3），由 provider 决定
    pub target_audio_format: String,

    // --- 进度上报（内部使用，不参与 Debug/Serialize） ---
    #[doc(hidden)]
    pub reporter: Arc<dyn ProgressReporter + Send + Sync>,
    /// ffmpeg/ffprobe 额外搜索目录（CLI: exe 同目录；Tauri: resource_dir）
    pub extra_bin_dirs: Vec<PathBuf>,
}

// 手动 Clone（跳过 reporter 的 trait object depth）
impl Clone for Config {
    fn clone(&self) -> Self {
        Config {
            provider: self.provider,
            api_key: self.api_key.clone(),
            resource_id: self.resource_id.clone(),
            legacy_mode: self.legacy_mode,
            app_key: self.app_key.clone(),
            access_key: self.access_key.clone(),
            azure_key: self.azure_key.clone(),
            azure_region: self.azure_region.clone(),
            language: self.language.clone(),
            enable_speaker_info: self.enable_speaker_info,
            enable_itn: self.enable_itn,
            enable_punc: self.enable_punc,
            enable_ddc: self.enable_ddc,
            enable_auto_lang: self.enable_auto_lang,
            show_utterances: self.show_utterances,
            end_window_size: self.end_window_size,
            boosting_table_name: self.boosting_table_name.clone(),
            correct_table_name: self.correct_table_name.clone(),
            corpus_context: self.corpus_context.clone(),
            ark_model: self.ark_model.clone(),
            las_region: self.las_region.clone(),
            operator_version: self.operator_version.clone(),
            model_version: self.model_version.clone(),
            enable_denoise: self.enable_denoise,
            enable_multi_language: self.enable_multi_language,
            enable_channel_split: self.enable_channel_split,
            show_speech_rate: self.show_speech_rate,
            show_volume: self.show_volume,
            enable_lid: self.enable_lid,
            enable_emotion_detection: self.enable_emotion_detection,
            enable_gender_detection: self.enable_gender_detection,
            sensitive_words_filter: self.sensitive_words_filter.clone(),
            enable_poi_fc: self.enable_poi_fc,
            enable_music_fc: self.enable_music_fc,
            candidate_locales: self.candidate_locales.clone(),
            word_level_timestamps: self.word_level_timestamps,
            profanity_filter_mode: self.profanity_filter_mode.clone(),
            punctuation_mode: self.punctuation_mode.clone(),
            output_dir: self.output_dir.clone(),
            poll_interval_secs: self.poll_interval_secs,
            max_duration_secs: self.max_duration_secs,
            max_size_bytes: self.max_size_bytes,
            prepare_only: self.prepare_only,
            max_split_depth: self.max_split_depth,
            target_audio_format: self.target_audio_format.clone(),
            reporter: Arc::clone(&self.reporter),
            extra_bin_dirs: self.extra_bin_dirs.clone(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("api_key", &"[redacted]")
            .field("resource_id", &self.resource_id)
            .field("language", &self.language)
            .field("output_dir", &self.output_dir)
            .field("max_duration_secs", &self.max_duration_secs)
            .field("max_size_bytes", &self.max_size_bytes)
            .field("prepare_only", &self.prepare_only)
            .field("target_audio_format", &self.target_audio_format)
            .field("extra_bin_dirs", &self.extra_bin_dirs)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// 音频输入/探测/准备
// ---------------------------------------------------------------------------

/// 用户提供的音频输入（解析后）
#[derive(Debug, Clone)]
pub struct AudioInput {
    /// 用户输入的原始字符串（URL 或本地路径）
    pub original: String,
    /// 本地文件路径（URL 会先下载到临时目录）
    pub source_path: PathBuf,
    /// 是否为 HTTP(S) URL
    pub is_url: bool,
    /// 若为 URL，这是提交给 API 的地址；若为本地文件则为 None
    pub submission_url: Option<String>,
}

/// ffprobe 探测结果
#[derive(Debug, Clone)]
pub struct ProbeMeta {
    pub format_name: String,
    pub codec_name: String,
    pub sample_rate: u32,
    pub bitrate_bps: u64,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub duration_secs: f64,
    pub size_bytes: u64,
}

/// 准备就绪的音频片段（可供提交或本地保存）
#[derive(Debug, Clone)]
pub struct PreparedChunk {
    /// 片段本地路径
    pub path: PathBuf,
    /// 容器格式（wav / mp3 / ogg / raw）
    pub format: String,
    /// 编码格式
    pub codec: String,
    /// 采样率
    pub sample_rate: u32,
    /// 时长（秒）
    pub duration_secs: f64,
    /// 文件大小（字节）
    pub size_bytes: u64,
    /// 提交给 API 使用的 URL（仅当输入为 URL 时有值）
    pub submission_url: Option<String>,
}

// ---------------------------------------------------------------------------
// 汇总/持久化类型
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PersistedSummary {
    pub original_input: String,
    pub source_path: String,
    pub is_url: bool,
    pub chunks: Vec<ChunkSummary>,
    pub submitted: Vec<SubmittedTaskSummary>,
}

#[derive(Debug, Serialize)]
pub struct ChunkSummary {
    pub path: String,
    pub format: String,
    pub codec: String,
    pub sample_rate: u32,
    pub duration_secs: f64,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct SubmittedTaskSummary {
    pub request_id: String,
    pub chunk_path: String,
    pub submission_url: String,
    pub status_code: Option<i64>,
    pub status_message: Option<String>,
    pub result_text: Option<String>,
    pub result_json_path: Option<String>,
}
