//! 编译时注入 git 版本信息到环境变量 GIT_VERSION
//!
//! 在非 git 环境（如用户下载的源码包）下回退到 Cargo.toml 版本号。

use std::process::Command;

fn main() {
    let git_version = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));

    println!("cargo:rustc-env=GIT_VERSION={git_version}");
}
