//! 火山引擎 TOS（对象存储）上传模块
//!
//! 将本地音频文件上传到 TOS，生成 `tos://` 内部 URL（供 LAS 和 bigmodel 使用）。
//!
//! SDK: ve-tos-rust-sdk v2.9.2
//! 文档: https://github.com/volcengine/ve-tos-rust-sdk

use anyhow::{anyhow, Result};
use std::path::Path;
use std::time::Duration;

use ve_tos_rust_sdk::asynchronous::object::ObjectAPI;
use ve_tos_rust_sdk::asynchronous::tos;
use ve_tos_rust_sdk::asynchronous::tos::AsyncRuntime;
use ve_tos_rust_sdk::object::PutObjectFromFileInput;

use async_trait::async_trait;
use futures_core::future::BoxFuture;
use std::future::Future;

// ---------------------------------------------------------------------------
// TokioRuntime — 适配 ve-tos-rust-sdk 的 AsyncRuntime trait
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct TokioRuntime {}

#[async_trait]
impl AsyncRuntime for TokioRuntime {
    type JoinError = tokio::task::JoinError;

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn spawn<'a, F>(&self, future: F) -> BoxFuture<'a, Result<F::Output, Self::JoinError>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Box::pin(tokio::runtime::Handle::current().spawn(future))
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        tokio::runtime::Handle::current().block_on(future)
    }
}

// ---------------------------------------------------------------------------
// TOS 客户端包装（泛型，因为 tos::Client 是 trait）
// ---------------------------------------------------------------------------

pub struct TosUploader<C> {
    client: C,
    bucket: String,
    endpoint: String,
}

/// 创建 TOS 上传器。返回通用类型，由调用方持有。
pub fn create_tos_uploader(
    ak: &str,
    sk: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
) -> Result<TosUploader<impl tos::TosClient>> {
    let client = tos::builder::<TokioRuntime>()
        .connection_timeout(10_000)
        .request_timeout(300_000)
        .max_connections(8)
        .max_retry_count(3)
        .ak(ak)
        .sk(sk)
        .region(region)
        .endpoint(format!("https://{endpoint}"))
        .enable_crc(true)
        .auto_recognize_content_type(true)
        .build()
        .map_err(|e| anyhow!("创建 TOS 客户端失败: {e}"))?;

    Ok(TosUploader {
        client,
        bucket: bucket.to_string(),
        endpoint: endpoint.to_string(),
    })
}

impl<C: tos::TosClient> TosUploader<C> {
    /// 上传本地文件到 TOS，返回 `tos://` 内部路径 URL（LAS 和 bigmodel 都可使用）
    pub async fn upload_file(&self, file_path: &Path, remote_key: &str) -> Result<TosUploadResult> {
        let mut input = PutObjectFromFileInput::new(&self.bucket, remote_key);
        input.set_file_path(file_path.to_string_lossy().to_string());
        if let Some(mime) = guess_mime(file_path) {
            input.set_content_type(mime);
        }

        let output = self.client.put_object_from_file(&input).await.map_err(|e| {
            anyhow!("TOS 上传失败: {e}")
        })?;

        println!(
            "   ✅ TOS 上传完成: {}  →  tos://{}/{}",
            file_path.file_name().unwrap_or_default().to_string_lossy(),
            self.bucket,
            remote_key
        );

        Ok(TosUploadResult {
            request_id: output.request_id().to_string(),
            bucket: self.bucket.clone(),
            key: remote_key.to_string(),
            tos_url: format!("tos://{}/{}", self.bucket, remote_key),
            https_url: format!(
                "https://{}.{}/{}",
                self.bucket, self.endpoint, remote_key
            ),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TosUploadResult {
    pub request_id: String,
    pub bucket: String,
    pub key: String,
    /// tos:// 内部 URL —— LAS 和 bigmodel API 都可使用
    pub tos_url: String,
    /// 公网 HTTP URL —— 用于 bigmodel API（无需签名时）
    pub https_url: String,
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

fn guess_mime(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str())?.to_lowercase().as_str() {
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mpeg"),
        "ogg" | "opus" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "flac" => Some("audio/flac"),
        "aac" => Some("audio/aac"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        _ => None,
    }
}
