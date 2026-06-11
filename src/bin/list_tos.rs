use ve_tos_rust_sdk::asynchronous::object::ObjectAPI;
use ve_tos_rust_sdk::asynchronous::tos;
use ve_tos_rust_sdk::asynchronous::tos::AsyncRuntime;
use ve_tos_rust_sdk::object::ListObjectsType2Input;
use async_trait::async_trait;
use futures_core::future::BoxFuture;
use std::future::Future;
use std::time::Duration;

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
    let ak = "AKLTZTc1MGIxOTU2OTZhNDQwMjhhOTg2ZjgyZDAwMzJiOGE";
    let sk = "TnpBd016TXlNVGhqTXpaak5HSXhOVGczWVdVNU1XRm1NemcwT1RZMFpqaw==";
    let c = tos::builder::<TR>().ak(ak).sk(sk).region("cn-beijing").endpoint("https://tos-cn-beijing.volces.com").build().unwrap();
    let mut input = ListObjectsType2Input::new("amamizu-oss");
    input.set_max_keys(1000);
    match c.list_objects_type2(&input).await {
        Ok(o) => {
            println!("Bucket: amamizu-oss | {} objects\n", o.contents().len());
            for obj in o.contents() {
                let s = obj.size();
                let size_str = if s >= 1048576 { format!("{:>5.1}MB", s as f64/1048576.0) } else if s >= 1024 { format!("{:>5}KB", s/1024) } else { format!("{:>5}B", s) };
                let modified = obj.last_modified().map(|d| format!("{d}")).unwrap_or_else(|| "?".into());
                println!("{}  {}  {}", size_str, modified, obj.key());
            }
        }
        Err(e) => eprintln!("Error: {e}"),
    }
}
