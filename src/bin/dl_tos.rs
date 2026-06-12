/// 从 TOS 下载文件（使用 get_object_to_file）
use ve_tos_rust_sdk::asynchronous::tos;
use ve_tos_rust_sdk::asynchronous::tos::AsyncRuntime;
use ve_tos_rust_sdk::asynchronous::object::ObjectAPI;
use ve_tos_rust_sdk::object::GetObjectToFileInput;
use async_trait::async_trait;
use futures_core::future::BoxFuture;
use std::future::Future;
use std::time::Duration;
use std::fs;

#[derive(Debug, Default)] struct TR {}
#[async_trait]
impl AsyncRuntime for TR {
    type JoinError = tokio::task::JoinError;
    async fn sleep(&self, d: Duration) { tokio::time::sleep(d).await; }
    fn spawn<'a, F>(&self, f: F) -> BoxFuture<'a, Result<F::Output, Self::JoinError>> where F: Future + Send + 'static, F::Output: Send + 'static { Box::pin(tokio::runtime::Handle::current().spawn(f)) }
    fn block_on<F: Future>(&self, f: F) -> F::Output { tokio::runtime::Handle::current().block_on(f) }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: dl_tos <tos_key> <local_path>");
        std::process::exit(1);
    }
    let key = &args[1];
    let local = &args[2];

    let ak = "AKLTZTc1MGIxOTU2OTZhNDQwMjhhOTg2ZjgyZDAwMzJiOGE";
    let sk = "TnpBd016TXlNVGhqTXpaak5HSXhOVGczWVdVNU1XRm1NemcwT1RZMFpqaw==";

    let c = tos::builder::<TR>()
        .ak(ak).sk(sk)
        .region("cn-beijing")
        .endpoint("https://tos-cn-beijing.volces.com")
        .build().unwrap();

    if let Some(parent) = std::path::Path::new(local).parent() {
        fs::create_dir_all(parent).unwrap();
    }

    println!("Downloading: {}", key);
    let input = GetObjectToFileInput::new("amamizu-oss", key, local);
    let output = c.get_object_to_file(&input).await.unwrap();
    let sz = fs::metadata(local).unwrap().len();
    println!("Done: {} MB (status: {})", sz / 1048576, output.status_code());
}
