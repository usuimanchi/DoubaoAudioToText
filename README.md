# AnyModelAudioToText

多平台大模型语音转文字客户端。支持 Windows / macOS，调用火山方舟豆包、Azure 等大模型能力，音频拖入即可转写。

> 📦 [最新版下载](https://github.com/usuimanchi/AnyModelAudioToText/releases/latest)

---

## 使用方式

### 图形界面（推荐）

下载对应平台的 zip → 解压 → 双击运行。

| 平台 | 下载 |
|------|------|
| Windows | `AnyModelAudioToText-*-windows.zip` |
| macOS | `AnyModelAudioToText-*-macos-arm64.zip` |

1. 粘贴 API Key（以 `ark-` 开头）
2. 拖拽音频文件到窗口（MP3 / WAV / M4A / AAC / FLAC 等）
3. 点「开始转写」，实时查看进度
4. ffmpeg 缺失时自动下载（国内用户走清华镜像源）

### 命令行

```bash
# 图形界面
doubao-transcriber

# 命令行模式
volc_auc_batch_client --api-key "ark-..." --inputs "音频.m4a"
```

---

## 支持的提供商

| 提供商 | 模型 | 不限时长 |
|--------|------|---------|
| **Ark** (火山方舟豆包) ⭐默认 | doubao-seed-2-0-lite | ≤2h |
| LAS (AI数据湖) | Seed-ASR | ✅ |
| Volcengine (录音文件识别) | bigmodel | ≤30min |
| Azure (Speech-to-Text) | Speech-to-Text | ≤4h |

---

## 构建

```bash
# CLI（4MB，轻量，无需 Tauri）
cargo build --release -p volc_auc_batch_client

# GUI（22MB）
cargo build --release -p doubao-transcriber
```

前置：Rust 工具链、Node.js（仅 GUI 需要 Tauri）、MSVC/Clang。

---

## 项目结构

```
core/          — 核心 lib（音频处理、后端编排、进度抽象）
cli/           — CLI 入口（clap + dialoguer）
src-tauri/     — Tauri v2 GUI
frontend/      — 纯静态前端（HTML/JS/CSS）
```

---

## 文档

- [使用手册](USAGE.md) — GUI / CLI 详细说明
- [更新说明](CHANGELOG.md) — 版本变更记录

## License

[GPLv3](LICENSE)
