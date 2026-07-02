//! 转录后端 trait 和通用类型

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use crate::types::{Config, PreparedChunk, SubmittedTaskSummary};

// ---------------------------------------------------------------------------
// JobHandle —— 标识已提交任务的跨提供商句柄
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JobHandle {
    /// 提供商内部的任务标识符（火山引擎 UUID / Azure job ID）
    pub id: String,
    /// 用于查询状态的完整 URL（Azure 需要；火山引擎为 None 表示用固定 URL + header）
    pub query_url: Option<String>,
    /// 提供商
    pub provider: crate::types::Provider,
    /// LAS 算子的实际版本（提交成功后记录，供轮询使用）
    pub operator_version: Option<String>,
}

// ---------------------------------------------------------------------------
// TranscriptionOutput —— 等待完成后的统一输出
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TranscriptionOutput {
    /// 原始响应 JSON（用于存档）
    pub raw_json: Value,
    /// 提取出的纯文本（best-effort）
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// TranscriptionBackend trait
// ---------------------------------------------------------------------------

/// 语音转文本后端的核心抽象。
///
/// 每个提供商实现三个操作：
/// 1) `submit`     —— 提交一个音频片段
/// 2) `wait`       —— 轮询直到任务完成，返回统一输出
/// 3) `save_result` —— 持久化结果到磁盘
#[async_trait]
pub trait TranscriptionBackend: Send + Sync {
    /// 获取提供商名称（用于日志/显示）
    fn provider_name() -> &'static str
    where
        Self: Sized;

    /// 1. 提交一个音频片段，返回任务句柄
    async fn submit(
        client: &Client,
        config: &Config,
        chunk: &PreparedChunk,
    ) -> Result<JobHandle>;

    /// 2. 轮询直到任务完成，返回识别输出
    async fn wait_for_completion(
        client: &Client,
        config: &Config,
        handle: &JobHandle,
    ) -> Result<TranscriptionOutput>;

    /// 3. 将 JSON 和文本持久化到磁盘，返回汇总记录
    fn save_result(
        config: &Config,
        handle: &JobHandle,
        output: &TranscriptionOutput,
        chunk: &PreparedChunk,
    ) -> Result<SubmittedTaskSummary>;
}
