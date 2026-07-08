# AnyModelAudioToText 使用手册

## 这个工具是做什么的

把你电脑上的录音/音频文件自动转成文字。支持中文、法语、英语混合音频。

---

## 使用方法 — 图形界面（推荐）

### Windows

1. 下载 `AnyModelAudioToText-v*-windows.zip`，解压
2. 双击 `doubao-transcriber.exe`

### macOS

1. 下载 `AnyModelAudioToText-v*-macos-arm64.zip`，解压
2. 运行 `./doubao-transcriber`

### 操作步骤

**第 1 步：选择提供商（默认火山方舟豆包）**

直接使用默认的「火山方舟豆包」即可，这是最快最准确的。

**第 2 步：输入 API Key**

在密码框里粘贴你的 Ark API Key（以 `ark-` 开头）。勾选「记住 API Key」，下次打开就不必再输入了。

**第 3 步：选择语言（可选）**

默认自动识别，不选即可。如果要指定语言，点选对应的芯片（可多选）。

**第 4 步：添加音频文件**

把文件从文件夹拖到灰色虚线区域，或点击「选择文件」。支持常见格式：MP3 / WAV / M4A / AAC / OGG / FLAC。

**第 5 步：开始转写**

点击底部「🚀 开始转写」按钮。下方的日志区会实时显示处理进度。完成后可点击「打开」查看结果文件。

> ffmpeg 首次运行时会自动下载。国内用户自动走清华镜像源。

---

## 使用方法 — 命令行

```bash
# 双击运行 volc_auc_batch_client.exe（Windows）按提示输入
# 或直接传参
volc_auc_batch_client \
  --api-key "ark-..." \
  --inputs "音频.m4a"

# 仅检查/转换，不提交
volc_auc_batch_client --inputs "音频.m4a" --prepare-only
```

### 主要参数

| 参数 | 默认 | 说明 |
|------|------|------|
| `--provider` | ark | ark / las / volcengine / azure |
| `--api-key` | — | API Key（必填） |
| `--inputs` | — | 音频文件或 URL |
| `--language` | 自动 | zh-CN / fr-FR 等 |
| `--prepare-only` | false | 仅检查转换，不提交 |
| `--output-dir` | 桌面 | 输出目录 |

---

## 支持的格式

MP3 / WAV / M4A / AAC / OGG / FLAC / MP4 / WebM / Opus

---

## 输出

- `result_{文件名}.txt` — 转写文本
- `result.srt` — 字幕文件（LAS 后端支持）
- `manifest.json` — 处理汇总

---

## 小技巧

- 💡 把音频文件直接拖到窗口里，路径会自动填好
- 💡 第一次使用后，API Key 会被记住，下次无需重新输入
- 💡 处理大文件时不要关窗口，等出现"🎉 全部完成"即可
- 💡 也可以通过命令行传参：`--api-key "xxx" --inputs "音频路径"`
- 💡 支持网络 URL 作为输入：`--inputs "https://example.com/audio.mp3"`

---

## 常见问题

### Q: API Key 从哪里获取？

A: 去 [火山引擎 Ark 控制台](https://console.volcengine.com/ark/) 注册。登录后点「API Key 管理」→「创建 API Key」→ 复制。Key 以 `ark-` 开头。

### Q: 报错 "ffmpeg" 或 "ffprobe" 找不到？

A: 程序首次运行时会自动下载 ffmpeg。如果自动下载失败，可以手动把 `ffmpeg.exe`（或 `ffmpeg`）和 `ffprobe.exe`（或 `ffprobe`）放到程序同目录下。

### Q: 支持哪些音频格式？单个文件最大多大？

A: M4A、MP3、WAV、AAC、OGG、FLAC 等都支持。不支持的格式会自动转换。单个文件最大支持 512 MB（火山方舟豆包）或 25 MB（其他后端）。

### Q: 一个 3 小时的录音要多久？

A: 转换约 2 分钟，AI 转写约 10-15 分钟，总共不到 20 分钟。

### Q: 如果想自动化批量处理怎么办？

A: GUI 可以拖入多个文件一起处理。CLI 同样支持多文件输入。结果文件会自动以原音频文件名命名，同一目录下多个音频不会互相覆盖。

### Q: 多音频转写后的结果如何区分？

A: 结果文件会自动以原音频文件名命名，如 `result_访谈录音.txt`。同一目录下多个音频不会互相覆盖。
