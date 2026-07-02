//! TOS 上传模块 — 仅用于 LAS/Volcengine 后端的本地文件上传

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

#[derive(Debug, Default)]
struct TokioRuntime {}

#[async_trait]
impl AsyncRuntime for TokioRuntime {
    type JoinError = tokio::task::JoinError;
    async fn sleep(&self, d: Duration) { tokio::time::sleep(d).await; }
    fn spawn<'a, F>(&self, f: F) -> BoxFuture<'a, Result<F::Output, Self::JoinError>>
    where F: Future + Send + 'static, F::Output: Send + 'static {
        Box::pin(tokio::runtime::Handle::current().spawn(f))
    }
    fn block_on<F: Future>(&self, f: F) -> F::Output { tokio::runtime::Handle::current().block_on(f) }
}

pub struct TosUploader<C> {
    client: C,
    bucket: String,
}

pub fn create_tos_uploader(ak: &str, sk: &str, region: &str, endpoint: &str, bucket: &str) -> Result<TosUploader<impl tos::TosClient>> {
    let client = tos::builder::<TokioRuntime>()
        .ak(ak).sk(sk)
        .region(region)
        .endpoint(format!("https://{endpoint}"))
        .build()
        .map_err(|e| anyhow!("创建 TOS 客户端失败: {e}"))?;
    Ok(TosUploader { client, bucket: bucket.to_string() })
}

impl<C: tos::TosClient> TosUploader<C> {
    /// 上传文件到 TOS，返回 tos:// URL
    pub async fn upload(&self, file_path: &Path, remote_key: &str) -> Result<String> {
        let mut input = PutObjectFromFileInput::new(&self.bucket, remote_key);
        input.set_file_path(file_path.to_string_lossy().to_string());
        self.client.put_object_from_file(&input).await
            .map_err(|e| anyhow!("TOS 上传失败: {e}"))?;
        Ok(format!("tos://{}/{}", self.bucket, remote_key))
    }
}
