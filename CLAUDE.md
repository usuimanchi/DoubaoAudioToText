# CLAUDE.md

## 开发流程（Trunk-Based）

日常开发在 `main` 分支。发版时打 tag 推送，CI 自动构建并创建 Release。

### 版本号规则

- **x.y.z**：x（主版本）和 y（次版本）由用户手动决定，用于功能级别区分
- **z（补丁版本）**可自动递增：修复 bug、小幅改进时自增
- 当前版本：v0.3.2，下个补丁为 v0.3.3

## 版本发布流程

1. **更新 `CHANGELOG.md`** — 面向用户，说明新增/变更/修复的功能
2. **更新 `Cargo.toml`** workspace 版本号
3. **打 git tag**（如 `v0.4.0`）
4. **`git push --tags`** — CI 自动构建 Windows + Mac，创建 GitHub Release
5. 验证 **https://github.com/usuimanchi/AnyModelAudioToText/releases/latest**

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
├── USAGE.md                 # 使用手册（GitHub 渲染，CI 打包为 .txt）
├── CHANGELOG.md              # 更新说明
└── dist/                     # 本地构建产物（gitignored，CI 自动发布）
```

> 本地目录名仍为 `DoubaoAudioToText`，GitHub 仓库名已改为 `AnyModelAudioToText`。

## 构建

```bash
# CLI
cargo build --release -p volc_auc_batch_client

# GUI
cargo build --release -p doubao-transcriber
```

## 进度上报抽象

所有 println!/ProgressBar 通过 `Config.reporter: Arc<dyn ProgressReporter>` 注入：
- CLI: `CliProgressReporter` — println → stdout + indicatif 进度条
- Tauri GUI: `TauriProgressReporter` — emit "progress" 事件到前端
- 测试: `NoopReporter`

## 已知问题

- rustc 1.96.0 MSVC 构建脚本 linker 异常（`link: extra operand`）。用前版本 artifact 缓存可编译，clean build 需降级 rustc 或等修复。
