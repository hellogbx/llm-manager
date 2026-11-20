//! Launching, stopping and inspecting llama-server processes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{Config, ServiceDef};
use crate::util;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Running {
    pub pid: u32,
    pub started_at: u64,
    pub log: PathBuf,
    pub port: u16,
    pub model: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateStore {
    #[serde(default)]
    pub services: HashMap<String, Running>,
}

impl StateStore {
    pub fn path() -> PathBuf {
        util::app_dir().join("state.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Drop entries whose process disappeared.
    pub fn prune(&mut self) {
        self.services.retain(|_, run| is_alive(run.pid));
    }
}

pub fn logs_dir() -> PathBuf {
    util::app_dir().join("logs")
}

pub fn is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let result = libc::kill(pid as libc::pid_t, 0);
        result == 0
    }
}

pub fn port_in_use(host: &str, port: u16) -> bool {
    let target = format!("{host}:{port}");
    if let Ok(addrs) = target.to_socket_addrs() {
        for addr in addrs {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
                return true;
            }
        }
        return false;
    }
    false
}

/// Minimal HTTP/1.1 GET, enough to talk to llama-server's local endpoints.
pub fn http_get(host: &str, port: u16, path: &str, timeout_ms: u64) -> Result<String, String> {
    let target = format!("{host}:{port}");
    let addrs = target.to_socket_addrs()
        .map_err(|e| format!("{target}: {e}"))?
        .collect::<Vec<SocketAddr>>();
    let addr = *addrs.first().ok_or("无法解析地址")?;
    let timeout = Duration::from_millis(timeout_ms);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > 8 << 20 {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => break,
            Err(err) => return Err(err.to_string()),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = match text.find("\r\n\r\n") {
        Some(idx) => (&text[..idx], &text[idx + 4..]),
        None => return Err("响应不完整".into()),
    };
    let status_ok = head
        .lines()
        .next()
        .map(|line| line.contains(" 200") || line.contains(" 20"))
        .unwrap_or(false);
    if !status_ok {
        let first = head.lines().next().unwrap_or("?");
        return Err(format!("HTTP {first}"));
    }
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        Ok(dechunk(body))
    } else {
        Ok(body.to_string())
    }
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let line_end = match rest.find("\r\n") {
            Some(idx) => idx,
            None => break,
        };
        let size_text = rest[..line_end].trim();
        let size = match usize::from_str_radix(size_text, 16) {
            Ok(size) => size,
            Err(_) => break,
        };
        rest = &rest[line_end + 2..];
        if size == 0 {
            break;
        }
        if rest.len() < size {
            out.push_str(rest);
            break;
        }
        out.push_str(&rest[..size]);
        rest = &rest[size.min(rest.len())..];
        if rest.starts_with("\r\n") {
            rest = &rest[2..];
        }
    }
    out
}

pub fn health(host: &str, port: u16) -> Result<Value, String> {
    let body = http_get(host, port, "/health", 1500)?;
    serde_json::from_str(&body).map_err(|e| format!("health: {e}"))
}

pub fn props(host: &str, port: u16) -> Result<Value, String> {
    let body = http_get(host, port, "/props", 1500)?;
    serde_json::from_str(&body).map_err(|e| format!("props: {e}"))
}

pub fn metrics_text(host: &str, port: u16) -> Result<String, String> {
    http_get(host, port, "/metrics", 1500)
}

/// Resident set size in bytes (native `proc_pidinfo`, no subprocess).
pub fn rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let read = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if read == size {
            Some(info.pti_resident_size)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    }
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let key = std::ffi::CString::new(name).ok()?;
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let ok = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            &mut value as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        Some(value)
    } else {
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct SysInfo {
    pub total_ram: u64,
    pub free_ram: u64,
    pub disk_free: u64,
    pub disk_total: u64,
}

pub fn sys_info() -> SysInfo {
    let total_ram = sysctl_u64("hw.memsize").unwrap_or(0);
    let page_size = sysctl_u64("hw.pagesize").unwrap_or(4096);
    let free_pages = sysctl_u64("vm.page_free_count").unwrap_or(0);
    let mut info = SysInfo {
        total_ram,
        free_ram: free_pages.saturating_mul(page_size),
        ..Default::default()
    };
    #[cfg(unix)]
    unsafe {
        let mut stat: libc::statfs = std::mem::zeroed();
        let path = std::ffi::CString::new(util::home().display().to_string()).unwrap_or_default();
        if libc::statfs(path.as_ptr(), &mut stat) == 0 {
            let block = stat.f_bsize as u64;
            info.disk_free = (stat.f_bavail as u64).saturating_mul(block);
            info.disk_total = (stat.f_blocks as u64).saturating_mul(block);
        }
    }
    info
}

/// Start a service; returns the child pid.
pub fn start(config: &Config, service: &ServiceDef, state: &mut StateStore) -> Result<u32, String> {
    if let Some(run) = state.services.get(&service.id) {
        if is_alive(run.pid) {
            return Err(format!("服务已在运行 (pid {})", run.pid));
        }
    }
    let server = config
        .llama_server
        .clone()
        .filter(|p| p.is_file())
        .or_else(crate::config::find_llama_server)
        .ok_or("未找到 llama-server，请在设置中指定路径")?;
    if !service.model.is_file() {
        return Err(format!("模型文件不存在: {}", service.model.display()));
    }
    if port_in_use(&service.host, service.port) {
        return Err(format!("端口 {} 已被占用", service.port));
    }

    std::fs::create_dir_all(logs_dir()).map_err(|e| e.to_string())?;
    let log_path = logs_dir().join(format!("{}.log", service.id));
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let _ = writeln!(
        log,
        "\n===== {} 启动 {} =====",
        util::now_unix(),
        service.name
    );

    let args = service.command_line(&server);
    let mut command = Command::new(&server);
    command
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log));

    // Bundled llama.cpp builds ship dylibs next to the binary.
    if let Some(dir) = server.parent() {
        let has_libs = std::fs::read_dir(dir)
            .map(|entries| {
                entries.flatten().any(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    name.starts_with("lib") && name.ends_with(".dylib")
                })
            })
            .unwrap_or(false);
        if has_libs {
            let mut paths = vec![dir.display().to_string()];
            if let Ok(existing) = std::env::var("DYLD_LIBRARY_PATH") {
                if !existing.is_empty() {
                    paths.push(existing);
                }
            }
            command.env("DYLD_LIBRARY_PATH", paths.join(":"));
        }
    }

    let child = command
        .spawn()
        .map_err(|e| format!("启动失败 {}: {e}", server.display()))?;
    let pid = child.id();
    state.services.insert(
        service.id.clone(),
        Running {
            pid,
            started_at: util::now_unix(),
            log: log_path,
            port: service.port,
            model: service.model.clone(),
        },
    );
    state.save()?;
    Ok(pid)
}

pub fn stop(id: &str, state: &mut StateStore) -> Result<(), String> {
    let run = match state.services.get(id) {
        Some(run) => run.clone(),
        None => return Ok(()),
    };
    if is_alive(run.pid) {
        unsafe {
            libc::kill(run.pid as libc::pid_t, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !is_alive(run.pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
        if is_alive(run.pid) {
            unsafe {
                libc::kill(run.pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    state.services.remove(id);
    state.save()?;
    Ok(())
}

/// Last `lines` lines of a service log.
pub fn log_tail(path: &Path, lines: usize) -> Vec<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return vec!["(暂无日志)".to_string()],
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(128 * 1024);
    if start > 0 {
        use std::io::Seek;
        let _ = file.seek(std::io::SeekFrom::Start(start));
    }
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let collected: Vec<&str> = text.lines().collect();
    let skip = collected.len().saturating_sub(lines);
    collected[skip..].iter().map(|s| s.to_string()).collect()
}
