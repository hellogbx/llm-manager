//! Discovering and downloading GGUF models from Hugging Face (or a mirror).
//!
//! Network access goes through `curl`: it is preinstalled on macOS, handles TLS,
//! redirects and range requests (resume) without adding a dependency.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::util;

pub const MIRROR: &str = "https://hf-mirror.com";
pub const OFFICIAL: &str = "https://huggingface.co";

/// Endpoints to try, in order. `None`/empty means "mirror first, then official".
pub fn candidates(preferred: Option<&str>) -> Vec<String> {
    match preferred.map(str::trim).filter(|s| !s.is_empty()) {
        Some(url) => vec![url.trim_end_matches('/').to_string()],
        None => vec![MIRROR.to_string(), OFFICIAL.to_string()],
    }
}

#[derive(Clone, Debug)]
pub struct HubModel {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
}

impl HubModel {
    pub fn owner(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.id.split_once('/').map(|(_, n)| n).unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug)]
pub struct HubFile {
    pub name: String,
    pub size: u64,
}

impl HubFile {
    pub fn is_mmproj(&self) -> bool {
        self.name.to_ascii_lowercase().contains("mmproj")
    }
}

pub fn quant_of(name: &str) -> String {
    crate::gguf::quant_from_name(name).unwrap_or_else(|| "?".into())
}

/// "18.2 MiB · Q4_0"，量化未知时只显示大小。
pub fn meta_line(size: u64, name: &str) -> String {
    let quant = quant_of(name);
    if quant == "?" {
        crate::util::human_bytes(size)
    } else {
        format!("{} · {}", crate::util::human_bytes(size), quant)
    }
}

// ------------------------------------------------------------------ parsing

pub fn parse_search(body: &str) -> Result<Vec<HubModel>, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| format!("目录格式异常: {err}"))?;
    let list = value.as_array().ok_or("目录格式异常: 期望数组")?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            if id.is_empty() {
                return None;
            }
            Some(HubModel {
                id,
                downloads: item.get("downloads").and_then(Value::as_u64).unwrap_or(0),
                likes: item.get("likes").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect())
}

pub fn parse_files(body: &str) -> Result<Vec<HubFile>, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| format!("仓库信息异常: {err}"))?;
    let siblings = value
        .get("siblings")
        .and_then(Value::as_array)
        .ok_or("仓库里没有文件列表")?;
    let mut files: Vec<HubFile> = siblings
        .iter()
        .filter_map(|item| {
            let name = item.get("rfilename")?.as_str()?.to_string();
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".gguf") {
                return None;
            }
            // `imatrix*.gguf` 是量化用的重要性矩阵，不是模型。
            if lower.contains("imatrix") {
                return None;
            }
            let size = item
                .get("size")
                .and_then(Value::as_u64)
                .or_else(|| {
                    item.get("lfs")
                        .and_then(|lfs| lfs.get("size"))
                        .and_then(Value::as_u64)
                })
                .unwrap_or(0);
            Some(HubFile { name, size })
        })
        .collect();
    // 模型本体按体积从小到大，视觉投影（mmproj）排在最后。
    files.sort_by_key(|f| (f.is_mmproj(), f.size));
    if files.is_empty() {
        return Err("该仓库没有 .gguf 文件".into());
    }
    Ok(files)
}

/// Preference order for the "install in one click" button.
const PREFERRED_QUANTS: [&str; 7] = [
    "q4_k_m", "q4_k_s", "q5_k_m", "q6_k", "q5_k_s", "q8_0", "q4_0",
];

pub fn recommend(files: &[HubFile]) -> Option<usize> {
    let main: Vec<usize> = (0..files.len())
        .filter(|i| !files[*i].is_mmproj())
        .collect();
    if main.is_empty() {
        return None;
    }
    for quant in PREFERRED_QUANTS {
        if let Some(idx) = main
            .iter()
            .copied()
            .find(|i| files[*i].name.to_ascii_lowercase().contains(quant))
        {
            return Some(idx);
        }
    }
    // 没有常见量化时取体积居中的那个，避免挑到极小的测试文件或超大的 F16。
    Some(main[main.len() / 2])
}

// -------------------------------------------------------------------- fetch

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchKind {
    Search,
    Files,
}

pub struct FetchResult {
    pub base: String,
    pub body: Result<String, String>,
}

fn request(kind: FetchKind, base: &str, arg: &str) -> String {
    match kind {
        FetchKind::Search => {
            let mut url =
                format!("{base}/api/models?filter=gguf&sort=downloads&direction=-1&limit=40");
            let query = arg.trim();
            if !query.is_empty() {
                url.push_str("&search=");
                url.push_str(&urlencode(query));
            }
            url
        }
        FetchKind::Files => format!("{base}/api/models/{}?blobs=true", arg),
    }
}

/// Run a catalog request on a background thread, trying each endpoint in turn.
pub fn spawn(kind: FetchKind, bases: Vec<String>, arg: String) -> Receiver<FetchResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut last_base = String::new();
        let mut last_err = String::from("没有可用的下载源");
        for base in &bases {
            last_base = base.clone();
            match http_get(&request(kind, base, &arg), 25) {
                Ok(body) => {
                    let _ = tx.send(FetchResult {
                        base: base.clone(),
                        body: Ok(body),
                    });
                    return;
                }
                Err(err) => last_err = err.to_string(),
            }
        }
        let _ = tx.send(FetchResult {
            base: last_base,
            body: Err(last_err),
        });
    });
    rx
}

/// 批量读取仓库文件清单，供列表展示总体积；结果按 `gen` 打标，过期批次直接丢弃。
/// 批量文件清单拉取的结果通道：`(批次号, 仓库 id, 文件列表)`。
pub type ListingRx = std::sync::mpsc::Receiver<(u64, String, Result<Vec<HubFile>, String>)>;

pub fn spawn_listing(base: String, ids: Vec<String>, gen: u64) -> ListingRx {
    let (tx, rx) = mpsc::channel();
    if ids.is_empty() {
        return rx;
    }
    let workers = ids.len().min(4);
    let chunk = ids.len().div_ceil(workers);
    for worker in 0..workers {
        let start = worker * chunk;
        let end = (start + chunk).min(ids.len());
        let slice = ids[start..end].to_vec();
        let tx = tx.clone();
        let base = base.clone();
        std::thread::spawn(move || {
            for id in slice {
                let url = format!("{base}/api/models/{id}?blobs=true");
                let files = http_get(&url, 20).and_then(|body| parse_files(&body));
                if tx.send((gen, id, files)).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);
    rx
}

/// 目标目录所在卷的剩余字节数。
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::ffi::CString;
    let c_path = CString::new(dir.to_string_lossy().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// 下载前检查本地条件：目录可建可写、剩余空间够用。
/// `Ok(Some(msg))` 是可以继续但值得提醒的情况（例如体积接近物理内存）。
pub fn preflight(dir: &Path, file: &str, size: u64) -> Result<Option<String>, String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("无法创建目录 {}：{err}", dir.display()))?;
    let probe = dir.join(".llmmgr-write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(err) => return Err(format!("目标目录不可写：{}（{err}）", dir.display())),
    }
    let margin = (size / 20).max(64 * 1024 * 1024);
    if let Some(free) = free_space(dir) {
        if free < size + margin {
            return Err(format!(
                "磁盘空间不足：{file} 需要 {}，{} 只剩 {}",
                util::human_bytes(size + margin),
                dir.display(),
                util::human_bytes(free)
            ));
        }
    }
    let ram = crate::runtime::sys_info().total_ram;
    if ram > 0 && size > ram - ram / 10 {
        return Ok(Some(format!(
            "{file} 有 {}，接近物理内存 {}，加载可能失败或很慢",
            util::human_bytes(size),
            util::human_bytes(ram)
        )));
    }
    Ok(None)
}

pub fn http_get(url: &str, timeout_secs: u64) -> Result<String, String> {
    let out = Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--max-time",
            &timeout_secs.to_string(),
            "-H",
            "User-Agent: llm-manager",
        ])
        .arg(url)
        .output()
        .map_err(|err| format!("无法执行 curl: {err}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let code = out.status.code().unwrap_or(-1);
        return Err(match (code, stderr.is_empty()) {
            (28, _) => "连接超时".to_string(),
            (6, _) => "无法解析域名".to_string(),
            (7, _) => "无法连接下载源".to_string(),
            (_, false) => stderr,
            _ => format!("请求失败（HTTP {code}）"),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ----------------------------------------------------------------- download

/// A single file download driven by a `curl` child process.
pub struct Download {
    pub model: String,
    pub file: String,
    pub dir: PathBuf,
    pub dest: PathBuf,
    part: PathBuf,
    pub total: u64,
    pub received: u64,
    pub speed: f64,
    pub error: Option<String>,
    pub done: bool,
    child: Option<Child>,
    last_tick: Instant,
    last_bytes: u64,
}

impl Download {
    pub fn key(&self) -> String {
        format!("{}/{}", self.model, self.file)
    }

    pub fn start(
        base: &str,
        model: &str,
        file: &str,
        dir: &Path,
        total: u64,
    ) -> Result<Self, String> {
        if file.contains('/') || file.contains("..") {
            return Err("文件名不合法".into());
        }
        std::fs::create_dir_all(dir).map_err(|err| format!("无法创建目录: {err}"))?;
        let dest = dir.join(file);
        if dest.is_file() {
            return Err("文件已存在".into());
        }
        let part = dir.join(format!("{file}.part"));
        let url = format!("{base}/{model}/resolve/main/{file}");
        let mut resume = std::fs::metadata(&part)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if resume {
            let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            // A leftover that is already bigger than the real file is corrupt:
            // drop it instead of resuming on top of garbage.
            if total > 0 && have >= total {
                let _ = std::fs::remove_file(&part);
                resume = false;
            }
        }

        let mut cmd = Command::new("curl");
        cmd.args([
            "-L",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "2",
            "--retry-delay",
            "2",
        ]);
        if resume {
            cmd.args(["-C", "-"]);
        }
        cmd.arg("-o").arg(&part).arg(&url);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = cmd.spawn().map_err(|err| format!("无法执行 curl: {err}"))?;
        let received = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            model: model.to_string(),
            file: file.to_string(),
            dir: dir.to_path_buf(),
            dest,
            part,
            total,
            received,
            speed: 0.0,
            error: None,
            done: false,
            child: Some(child),
            last_tick: Instant::now(),
            last_bytes: received,
        })
    }

    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.received as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    pub fn eta(&self) -> Option<u64> {
        if self.speed < 32.0 * 1024.0 || self.total == 0 {
            return None;
        }
        let left = self.total.saturating_sub(self.received) as f64;
        Some((left / self.speed).max(0.0) as u64)
    }

    pub fn cancel(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !self.done {
            self.done = true;
            self.error = Some("已取消（保留断点，再次下载会续传）".into());
        }
    }

    /// 退出应用时杀掉 curl，避免子进程变成孤儿继续写 `.part`。
    pub fn abort(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Sample progress. Returns true when the download state changed in a way
    /// worth reporting (finished, failed or cancelled).
    pub fn poll(&mut self) -> bool {
        if self.done {
            return false;
        }
        let mut status = None;
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(finished)) => status = Some(finished),
                Ok(None) => {}
                Err(err) => {
                    self.error = Some(format!("下载进程异常: {err}"));
                    self.done = true;
                    self.child = None;
                    return true;
                }
            }
        }
        if let Some(finished) = status {
            let stderr = self
                .child
                .take()
                .and_then(|child| child.wait_with_output().ok())
                .map(|out| String::from_utf8_lossy(&out.stderr).trim().to_string())
                .unwrap_or_default();
            self.received = std::fs::metadata(&self.part).map(|m| m.len()).unwrap_or(0);
            self.speed = 0.0;
            let complete = self.total == 0 || self.received == self.total;
            if finished.success() && !complete && self.received > self.total {
                let _ = std::fs::remove_file(&self.part);
                self.error = Some("下载数据异常，已清理临时文件，请重试".into());
            } else if finished.success() && !complete {
                self.error = Some(format!(
                    "下载数据不完整（已收到 {}，应为 {}）",
                    util::human_bytes(self.received),
                    util::human_bytes(self.total)
                ));
            } else if finished.success() {
                match std::fs::rename(&self.part, &self.dest) {
                    Ok(()) => {
                        self.error = None;
                        self.received = self.total.max(self.received);
                    }
                    Err(err) => self.error = Some(format!("无法保存文件: {err}")),
                }
            } else if !stderr.is_empty() {
                self.error = Some(stderr);
            } else {
                self.error = Some(format!(
                    "下载未完成（退出码 {}）",
                    finished.code().unwrap_or(-1)
                ));
            }
            self.done = true;
            return true;
        }

        self.received = std::fs::metadata(&self.part).map(|m| m.len()).unwrap_or(0);
        if self.total > 0 && self.received > self.total + 1024 {
            // Two writers on one .part (or a broken range response) always ends
            // in a corrupt file: stop now and clean up so a retry starts fresh.
            self.cancel();
            let _ = std::fs::remove_file(&self.part);
            self.error = Some("下载数据异常，已清理临时文件，请重试".into());
            return true;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick);
        if dt >= Duration::from_millis(300) {
            let delta = self.received.saturating_sub(self.last_bytes) as f64;
            let instant = delta / dt.as_secs_f64();
            self.speed = if self.speed <= 1.0 {
                instant
            } else {
                self.speed * 0.65 + instant * 0.35
            };
            self.last_tick = now;
            self.last_bytes = self.received;
        }
        false
    }
}

pub fn speed_label(speed: f64) -> String {
    if speed <= 1.0 {
        String::new()
    } else {
        format!("{}/s", util::human_bytes(speed as u64))
    }
}

pub fn eta_label(seconds: u64) -> String {
    if seconds >= 60 {
        format!("剩余 {} 分 {} 秒", seconds / 60, seconds % 60)
    } else {
        format!("剩余 {seconds} 秒")
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        self.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("llmmgr-hub-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn preflight_rejects_impossible_size() {
        let dir = temp_dir("space");
        let probe = dir.parent().unwrap().to_path_buf();
        let free = free_space(&probe).expect("free space");
        let err = preflight(&dir, "big.gguf", free * 2).unwrap_err();
        assert!(err.contains("磁盘空间不足"), "unexpected: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_accepts_small_file() {
        let dir = temp_dir("ok");
        assert!(preflight(&dir, "small.gguf", 1024 * 1024)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_warns_when_file_exceeds_memory() {
        let dir = temp_dir("ram");
        let ram = crate::runtime::sys_info().total_ram;
        let free = free_space(dir.parent().unwrap()).unwrap_or(0);
        if free < ram + 512 * 1024 * 1024 {
            return;
        }
        let warning = preflight(&dir, "huge.gguf", ram).unwrap();
        assert!(warning.unwrap().contains("内存"), "no memory warning");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
