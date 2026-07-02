//! 多提供商语音转文本核心库
//!
//! 提供音频处理、后端编排、进度上报，**不依赖** CLI 框架（clap/dialoguer）或 GUI 框架（tauri）。
//! CLI 和 Tauri GUI 都通过本库共享核心逻辑。

pub mod ark;
pub mod audio;
pub mod azure;
pub mod backend;
pub mod input;
pub mod las;
pub mod output;
pub mod pipeline;
pub mod progress;
pub mod tos_upload;
pub mod types;
pub mod volcengine;

// 重导出常用类型
pub use backend::{JobHandle, TranscriptionBackend, TranscriptionOutput};
pub use pipeline::{
    dedup_overlap, detect_system_lang, merge_chunk_results, output_stem, run_pipeline,
    run_pipeline_for_provider, sanitize_filename, sanitize_path,
};
pub use progress::{CliProgressReporter, NoopReporter, ProgressEvent, ProgressReporter};
pub use types::*;
