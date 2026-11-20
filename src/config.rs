//! Persistent configuration: model directories, llama-server location, services.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::util;

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_ctx() -> u32 {
    4096
}
fn default_ngl() -> i32 {
    99
}
fn default_parallel() -> u32 {
    1
}
fn default_refresh() -> u64 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDef {
    pub id: String,
    pub name: String,
    pub model: PathBuf,
    #[serde(default)]
    pub mmproj: Option<PathBuf>,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_ctx")]
    pub ctx: u32,
    #[serde(default = "default_ngl")]
    pub ngl: i32,
    #[serde(default = "default_parallel")]
    pub parallel: u32,
    #[serde(default)]
    pub image_min_tokens: Option<u32>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl ServiceDef {
    pub fn new(name: &str, model: &Path) -> Self {
        Self {
            id: slug(name),
            name: name.to_string(),
            model: model.to_path_buf(),
            mmproj: None,
            host: default_host(),
            port: default_port(),
            ctx: default_ctx(),
            ngl: default_ngl(),
            parallel: default_parallel(),
            image_min_tokens: None,
            extra_args: Vec::new(),
        }
    }

    /// The exact command line the manager would launch, for display and debugging.
    pub fn command_line(&self, server: &Path) -> Vec<String> {
        let mut args = vec![
            server.display().to_string(),
            "--model".into(),
            self.model.display().to_string(),
            "--host".into(),
            self.host.clone(),
            "--port".into(),
            self.port.to_string(),
            "-c".into(),
            self.ctx.to_string(),
            "-ngl".into(),
            self.ngl.to_string(),
            "-np".into(),
            self.parallel.to_string(),
            "--jinja".into(),
            "--no-webui".into(),
            "--metrics".into(),
        ];
        if let Some(mmproj) = &self.mmproj {
            args.push("--mmproj".into());
            args.push(mmproj.display().to_string());
            if let Some(tokens) = self.image_min_tokens {
                args.push("--image-min-tokens".into());
                args.push(tokens.to_string());
            }
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model_dirs: Vec<PathBuf>,
    pub llama_server: Option<PathBuf>,
    #[serde(default)]
    pub services: Vec<ServiceDef>,
    #[serde(default = "default_refresh")]
    pub refresh_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model_dirs: Vec::new(),
            llama_server: None,
            services: Vec::new(),
            refresh_secs: default_refresh(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        util::app_dir().join("config.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                eprintln!("配置解析失败, 使用默认配置: {err}");
                Self::detect()
            }),
            Err(_) => Self::detect(),
        }
    }

    /// Sensible first-run defaults: existing model folders and a llama-server on PATH.
    pub fn detect() -> Self {
        let home = util::home();
        let candidates = [
            home.join("models"),
            home.join("Models"),
            home.join("Downloads"),
            util::app_dir().join("models"),
        ];
        let model_dirs = candidates
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect::<Vec<_>>();
        Self {
            model_dirs,
            llama_server: find_llama_server(),
            services: Vec::new(),
            refresh_secs: default_refresh(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn service(&self, id: &str) -> Option<&ServiceDef> {
        self.services
            .iter()
            .find(|s| s.id == id || s.name == id)
    }

    pub fn next_free_port(&self) -> u16 {
        let mut port = 8080;
        while self.services.iter().any(|s| s.port == port) {
            port += 1;
        }
        port
    }

    pub fn add_service(&mut self, mut service: ServiceDef) {
        let mut id = slug(&service.name);
        let mut n = 2;
        while self.services.iter().any(|s| s.id == id) {
            id = format!("{}-{}", slug(&service.name), n);
            n += 1;
        }
        service.id = id;
        self.services.push(service);
    }
}

/// Turn an arbitrary name into a stable, filesystem-friendly id.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("service-{}", util::now_unix())
    } else {
        trimmed
    }
}

pub fn find_llama_server() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("LLAMA_SERVER") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut dirs: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    let home = util::home();
    dirs.push(home.join(".local/bin"));
    dirs.push(home.join("bin"));
    dirs.push(home.join("llama.cpp/build/bin"));
    for dir in dirs {
        let candidate = dir.join("llama-server");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
