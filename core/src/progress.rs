//! 进度上报抽象层
//!
//! 替代散布在各模块的 `println!`/`ProgressBar`，让核心逻辑可被 CLI（打印到 stdout）
//! 和 Tauri GUI（emit 事件到前端）复用。reporter 通过 `Config` 注入，无需改动后端 trait 签名。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

// ---------------------------------------------------------------------------
// 事件类型
// ---------------------------------------------------------------------------

/// 日志级别
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 结构化进度事件。GUI 端通过 serde 序列化后 emit 给前端。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// 普通日志行（替代 println!）
    Log { level: LogLevel, msg: String },
    /// 确定性进度（pos/len）
    Progress { key: String, pos: u64, len: u64 },
    /// 非确定性 spinner 启动
    SpinnerStart { key: String, msg: String },
    /// spinner 停止
    SpinnerStop { key: String },
    /// 最终结果路径
    Result { path: String },
}

// ---------------------------------------------------------------------------
// trait
// ---------------------------------------------------------------------------

pub trait ProgressReporter: Send + Sync {
    fn emit(&self, event: ProgressEvent);

    fn log(&self, msg: String) {
        self.emit(ProgressEvent::Log {
            level: LogLevel::Info,
            msg,
        });
    }

    fn warn(&self, msg: String) {
        self.emit(ProgressEvent::Log {
            level: LogLevel::Warn,
            msg,
        });
    }

    fn error(&self, msg: String) {
        self.emit(ProgressEvent::Log {
            level: LogLevel::Error,
            msg,
        });
    }
}

// ---------------------------------------------------------------------------
// 实现
// ---------------------------------------------------------------------------

/// 无输出（默认/测试用）
pub struct NoopReporter;

impl ProgressReporter for NoopReporter {
    fn emit(&self, _event: ProgressEvent) {}
}

/// CLI 实现：Log → println，Progress/Spinner → indicatif::ProgressBar
pub struct CliProgressReporter {
    bars: Mutex<HashMap<String, indicatif::ProgressBar>>,
}

impl CliProgressReporter {
    pub fn new() -> Self {
        Self {
            bars: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for CliProgressReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressReporter for CliProgressReporter {
    fn emit(&self, event: ProgressEvent) {
        match event {
            ProgressEvent::Log { msg, .. } => println!("{msg}"),
            ProgressEvent::Progress { key, pos, len } => {
                let mut map = self.bars.lock().unwrap();
                let pb = map.entry(key.clone()).or_insert_with(|| {
                    let pb = indicatif::ProgressBar::new(len);
                    pb.set_style(
                        indicatif::ProgressStyle::with_template(
                            "[{elapsed_precise}] {wide_bar} {pos}/{len}",
                        )
                        .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar()),
                    );
                    pb
                });
                pb.set_length(len);
                pb.set_position(pos);
            }
            ProgressEvent::SpinnerStart { key, msg } => {
                let mut map = self.bars.lock().unwrap();
                let pb = indicatif::ProgressBar::new_spinner();
                pb.set_message(msg);
                map.insert(key, pb);
            }
            ProgressEvent::SpinnerStop { key } => {
                let mut map = self.bars.lock().unwrap();
                if let Some(pb) = map.remove(&key) {
                    pb.finish_and_clear();
                }
            }
            ProgressEvent::Result { path } => println!("📝 结果已保存: {path}"),
        }
    }
}
