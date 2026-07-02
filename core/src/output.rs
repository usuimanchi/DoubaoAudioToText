//! 结果持久化与汇总

use anyhow::Result;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use crate::types::{ChunkSummary, Config, PersistedSummary, SubmittedTaskSummary};

// ---------------------------------------------------------------------------
// API Key 记忆
// ---------------------------------------------------------------------------

pub fn persist_api_key_hint(path: &Path, api_key: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = File::create(path)?;
    f.write_all(api_key.as_bytes())?;
    Ok(())
}

pub fn load_last_api_key_hint(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Manifest 写入
// ---------------------------------------------------------------------------

pub fn write_manifest(
    path: &Path,
    summaries: &[PersistedSummary],
    config: &Config,
) -> Result<()> {
    let json = serde_json::to_vec_pretty(summaries)?;
    fs::write(path, json)?;
    config
        .reporter
        .log(format!("📋 汇总清单已保存到: {}", path.display()));
    Ok(())
}
