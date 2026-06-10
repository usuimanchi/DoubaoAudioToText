# Volc AUC Batch Client

多提供商语音转文本批量客户端。支持火山引擎、Azure 等平台，自动完成音频格式检测、转换、切分、上传和转写。

---

## 支持的提供商

| 提供商 | 模型 | 类型 | 不限时长 | 中文标点 | 法语原文 | 默认 |
|--------|------|------|---------|---------|---------|------|
| **Ark** | doubao-seed-2-0-lite | 多模态 LLM | ≤120min | ✅ | ✅ | ⭐ |
| LAS | las_asr_pro (Seed-ASR) | 专用 ASR | ✅ | ❌ | ✅ | |
| Volcengine | bigmodel (Seed-ASR) | 专用 ASR | ❌ 30min | ✅ | ❌ | |
| Azure | Speech-to-Text | 专用 ASR | ❌ 4h | ✅ | ✅ | |

---

## 快速开始

### 安装依赖

```bash
# Rust 工具链
# Windows: https://rustup.rs
# macOS/Linux: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# ffmpeg
# Windows: winget install ffmpeg
# macOS: brew install ffmpeg
# Linux: apt install ffmpeg
```

### 编译

```bash
cd DoubaoAudioToText
cargo build --release
```

### 使用

```bash
# 最小命令（默认 Ark）
./target/release/volc_auc_batch_client \
  --api-key "your-ark-key" \
  --tos-ak "AKLTZTc1MG..." --tos-sk "..." \
  --inputs "path/to/audio.m4a"

# 使用其他提供商
./target/release/volc_auc_batch_client --provider las \
  --api-key "las-532227bf..." \
  --inputs "tos://bucket/path/audio.ogg"

# 仅检查/转换音频，不提交
./target/release/volc_auc_batch_client \
  --inputs "./audio.m4a" --prepare-only
```

---

## 工作流程

```
M4A → ffprobe 探测 → ffmpeg 转 MP3 → 超限则切分(重叠10s) → TOS 上传 → API 提交 → 保存结果
```

输出：
- `result.txt` — 纯文本
- `result.srt` — 字幕文件
- `result_formatted.md` — 语言分段 + 时间戳
- `response.json` — API 原始响应
- `result_merged.txt` — 多片段合并（自动去重）
- `manifest.json` — 处理汇总

---

## 主要参数

| 参数 | 默认 | 说明 |
|------|------|------|
| `--provider` | ark | ark / las / volcengine / azure |
| `--api-key` | — | API Key（必填） |
| `--inputs` | — | 音频 URL 或本地文件路径 |
| `--language` | 空=自动 | zh-CN / fr-FR 等 |
| `--tos-ak` / `--tos-sk` | — | TOS 对象存储凭证（本地文件自动上传时需要） |
| `--tos-bucket` | amamizu-oss | TOS 存储桶 |
| `--max-duration-secs` | 1800(Ark:7200) | 单片最大时长（秒） |
| `--max-size-bytes` | 512M(Ark:25M) | 单片最大大小（字节） |
| `--prepare-only` | false | 仅准备不提交 |
| `--output-dir` | ./auc_output | 输出目录（本地文件默认输出到源目录） |

---

## 环境变量

见 `.env` 示例文件：

```bash
ARK_API_KEY=ark-cd35887d...
TOS_ACCESS_KEY=AKLTZTc1MG...
TOS_SECRET_KEY=TnpBd016TX...
```

---

## 项目结构

```
src/
  main.rs          CLI + 编排
  types.rs         共享数据类型 + Provider 枚举
  backend.rs       TranscriptionBackend trait
  ark.rs           Ark 方舟后端
  las.rs           LAS 算子后端
  volcengine.rs    火山引擎 bigmodel 后端
  azure.rs         Azure Speech-to-Text 后端
  audio.rs         ffmpeg/ffprobe 音频处理
  input.rs         URL 下载 / 本地文件解析
  tos.rs           TOS 对象存储上传
  output.rs        结果持久化
```

---

## License

MIT
