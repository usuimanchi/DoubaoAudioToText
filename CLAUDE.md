# CLAUDE.md

## 版本发布流程

每次更新代码后发布新版本时：

1. **更新 `dist/更新说明.txt`** — 面向用户，说明新增/变更/修复的功能，不提技术细节
2. **更新 `Cargo.toml`** workspace 版本号
3. **打 git tag**（如 `v0.3.0`）
4. **`cargo build --release -p volc_auc_batch_client`** — 编译 CLI
5. **复制 CLI 到 dist**：`cp target/release/volc_auc_batch_client.exe dist/`
6. **`tauri build`**（在 `src-tauri/` 目录）— 编译 GUI（Windows: .exe + nsis；Mac: .app）
7. **更新 `dist/使用手册.txt`** — 如果 CLI 参数或默认值变化

## 提供商命名

| Provider | 显示名称 |
|----------|---------|
| Ark | 火山方舟豆包（volcengine ark doubao） |
| LAS | 火山引擎 AI数据湖服务（volcengine） |
| Volcengine | 火山方舟录音文件识别服务 |
| Azure | Azure Speech-to-Text |

## 项目结构（workspace）

```
DoubaoAudioToText/
├── Cargo.toml              # [workspace] members = ["core", "cli"]
├── core/                   # 核心 lib crate「volc_core」
│   ├── src/lib.rs          # 公开导出
│   ├── src/ark.rs          # Ark (doubao-seed-2-0-lite) 后端
│   ├── src/audio.rs        # ffmpeg/ffprobe 音频处理（resolve_ffmpeg 跨平台）
│   ├── src/backend.rs      # TranscriptionBackend trait
│   ├── src/pipeline.rs     # run_pipeline 编排、合并去重
│   ├── src/progress.rs     # ProgressReporter trait + 实现
│   └── src/{input,output,las,azure,volcengine,tos_upload,types}.rs
├── cli/                    # CLI bin「volc_auc_batch_client」
│   ├── src/main.rs         # Cli/clap、build_config、print_banner、交互式 gather
│   └── src/bin/dl_tos.rs   # TOS 下载工具（gitignored，含凭据）
├── src-tauri/              # Tauri v2 GUI「豆包语音转文字」（非 workspace member，单独构建）
│   ├── Cargo.toml  tauri.conf.json  build.rs  capabilities/default.json
│   ├── src/{lib,main,commands}.rs
│   └── binaries/           # ffmpeg/ffprobe (gitignored)
├── frontend/               # Tauri 前端（纯静态 HTML/JS/CSS）
│   └── index.html  app.js  style.css
└── dist/                   # 发布文件（CLI exe、ffmpeg.exe/ffprobe.exe、使用手册、更新说明）
```

## 构建

```bash
# CLI（Windows / Mac / Linux）
cargo build --release -p volc_auc_batch_client

# Tauri GUI（需 node.js + npm，先安装 tauri-cli）
npm install -g @tauri-apps/cli
cd src-tauri && npm install && tauri build
```

## 进度上报抽象

所有 println!/ProgressBar 通过 `Config.reporter: Arc<dyn ProgressReporter>` 注入：
- CLI: `CliProgressReporter` — println → stdout + indicatif 进度条
- Tauri GUI: `TauriProgressReporter` — emit "progress" 事件到前端
- 测试: `NoopReporter`

## 已知问题

- rustc 1.96.0 MSVC 构建脚本 linker 异常（`link: extra operand`）。用前版本 artifact 缓存可编译，clean build 需降级 rustc 或等修复。
