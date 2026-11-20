//! Scanning model folders for GGUF files, grouping shards, pairing vision projectors.

use std::path::{Path, PathBuf};

use crate::gguf::{self, GgufMeta};
use crate::util;

#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub path: PathBuf,
    pub file_name: String,
    /// Every file belonging to this model (split GGUF shards included).
    pub files: Vec<PathBuf>,
    pub size: u64,
    pub meta: Option<GgufMeta>,
    pub error: Option<String>,
    pub mmproj: Option<PathBuf>,
    pub modified: Option<u64>,
}

impl ModelEntry {
    pub fn display_name(&self) -> String {
        let name = &self.file_name;
        let stem = name.strip_suffix(".gguf").unwrap_or(name);
        stem.to_string()
    }

    pub fn quant(&self) -> String {
        match &self.meta {
            Some(meta) => meta.quant_label(&self.file_name),
            None => gguf::quant_from_name(&self.file_name).unwrap_or_else(|| "?".into()),
        }
    }

    pub fn params(&self) -> String {
        self.meta
            .as_ref()
            .map(|m| m.params_label())
            .unwrap_or_else(|| "?".into())
    }

    pub fn arch(&self) -> String {
        self.meta
            .as_ref()
            .map(|m| m.arch.clone())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "?".into())
    }

    pub fn all_files(&self) -> Vec<PathBuf> {
        let mut files = self.files.clone();
        if let Some(mmproj) = &self.mmproj {
            if !files.contains(mmproj) {
                files.push(mmproj.clone());
            }
        }
        files
    }

    pub fn total_size(&self) -> u64 {
        let mut size = self.size;
        if let Some(mmproj) = &self.mmproj {
            size += std::fs::metadata(mmproj).map(|m| m.len()).unwrap_or(0);
        }
        size
    }
}

fn collect_gguf(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            collect_gguf(&path, depth + 1, out);
        } else if file_type.is_file() && name.to_ascii_lowercase().ends_with(".gguf") {
            out.push(path);
        }
    }
}

/// `model-00002-of-00003.gguf` -> `model`
fn shard_base(name: &str) -> Option<String> {
    let lowered = name.to_ascii_lowercase();
    let of = lowered.find("-of-")?;
    let tail = &lowered[of + 4..];
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !tail[digits.len()..].starts_with(".gguf") {
        return None;
    }
    let head = &name[..of];
    let base = match head.rfind('-') {
        Some(idx) => &head[..idx],
        None => head,
    };
    Some(base.to_string())
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x.eq_ignore_ascii_case(y))
        .count()
}

fn pick_mmproj(model_file: &str, mmproj_files: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(usize, PathBuf)> = None;
    for candidate in mmproj_files {
        let name = candidate
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let prefix = common_prefix_len(model_file, &name);
        if prefix >= 6 && best.as_ref().map(|(len, _)| prefix > *len).unwrap_or(true) {
            best = Some((prefix, candidate.clone()));
        }
    }
    best.map(|(_, p)| p)
}

/// Scan every configured directory. Returns models and per-file errors.
pub fn scan(dirs: &[PathBuf]) -> (Vec<ModelEntry>, Vec<String>) {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            errors.push(format!("目录不存在: {}", dir.display()));
            continue;
        }
        collect_gguf(dir, 0, &mut paths);
    }
    paths.sort();
    paths.dedup();

    let mmproj: Vec<PathBuf> = paths
        .iter()
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase().contains("mmproj"))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let plain: Vec<PathBuf> = paths
        .iter()
        .filter(|p| !mmproj.contains(p))
        .cloned()
        .collect();

    // Group split files under their first shard.
    let mut groups: Vec<(String, PathBuf, Vec<PathBuf>)> = Vec::new();
    for path in &plain {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let key = match shard_base(&name) {
            Some(base) => format!("{}/{}", dir.display(), base),
            None => path.display().to_string(),
        };
        if let Some(group) = groups.iter_mut().find(|(k, _, _)| *k == key) {
            group.2.push(path.clone());
        } else {
            groups.push((key, path.clone(), vec![path.clone()]));
        }
    }

    let mut models = Vec::new();
    for (_key, first, mut files) in groups {
        files.sort();
        files.dedup();
        let file_name = first
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size: u64 = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
            .sum();
        let modified = files
            .first()
            .and_then(|f| std::fs::metadata(f).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let (meta, error) = match gguf::read_meta(&first) {
            Ok(meta) => (Some(meta), None),
            Err(err) => (None, Some(err)),
        };
        let dir = first.parent().map(Path::to_path_buf).unwrap_or_default();
        let local_mmproj: Vec<PathBuf> = mmproj
            .iter()
            .filter(|p| p.parent() == Some(dir.as_path()))
            .cloned()
            .collect();
        models.push(ModelEntry {
            path: first,
            file_name: file_name.clone(),
            files,
            size,
            meta,
            error,
            mmproj: pick_mmproj(&file_name, &local_mmproj),
            modified,
        });
    }
    models.sort_by(|a, b| b.size.cmp(&a.size));
    (models, errors)
}

/// Delete files, moving them to the Trash when possible. Returns freed bytes.
pub fn delete_files(paths: &[PathBuf]) -> Result<u64, String> {
    let trash = util::home().join(".Trash");
    let mut freed = 0u64;
    for path in paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "model.gguf".into());
        let mut target = trash.join(&name);
        if trash.is_dir() && target.exists() {
            target = trash.join(format!("{}-{}", util::now_unix(), name));
        }
        let moved = trash.is_dir() && std::fs::rename(path, &target).is_ok();
        if !moved {
            std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        freed += size;
    }
    Ok(freed)
}
