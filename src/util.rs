//! Small formatting and path helpers shared by the CLI and the UI.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn duration_label(seconds: u64) -> String {
    let (d, h, m, s) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60,
    );
    if d > 0 {
        format!("{d}天 {h}小时")
    } else if h > 0 {
        format!("{h}小时 {m}分")
    } else if m > 0 {
        format!("{m}分 {s}秒")
    } else {
        format!("{s}秒")
    }
}

pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let head: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Directory holding config, state and logs. `LLMMGR_HOME` overrides it.
pub fn app_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("LLMMGR_HOME") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("llm-manager")
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}
