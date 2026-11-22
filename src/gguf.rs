//! Minimal GGUF metadata reader: enough to describe a model file without loading it.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct GgufMeta {
    pub version: u32,
    pub arch: String,
    pub name: String,
    pub size_label: String,
    pub file_type: Option<u32>,
    pub tensor_count: u64,
    pub params: u64,
    pub context_length: u64,
    pub block_count: u64,
    pub embedding_length: u64,
}

const MAGIC: &[u8; 4] = b"GGUF";

#[allow(dead_code)]
enum Value {
    Text(String),
    UInt(u64),
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
}

struct Reader {
    inner: BufReader<File>,
}

impl Reader {
    fn take(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; n];
        self.inner.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> io::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> io::Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn skip(&mut self, n: u64) -> io::Result<()> {
        let mut left = n;
        let mut buf = [0u8; 8192];
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            self.inner.read_exact(&mut buf[..want])?;
            left -= want as u64;
        }
        Ok(())
    }

    fn string(&mut self) -> io::Result<String> {
        let len = self.u64()?;
        let bytes = self.take(len as usize)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn value(&mut self, kind: u32) -> io::Result<Value> {
        Ok(match kind {
            0 => Value::UInt(self.u8()? as u64),
            1 => Value::Int(self.u8()? as i8 as i64),
            2 => Value::UInt(self.u16()? as u64),
            3 => Value::Int(self.u16()? as i16 as i64),
            4 => Value::UInt(self.u32()? as u64),
            5 => Value::Int(self.u32()? as i32 as i64),
            6 => Value::Float(self.f32()? as f64),
            7 => Value::Bool(self.u8()? != 0),
            8 => Value::Text(self.string()?),
            9 => {
                let item = self.u32()?;
                let count = self.u64()?;
                // Arrays are only skipped (tokenizer vocab, merges, ...).
                for _ in 0..count {
                    if item == 8 {
                        let len = self.u64()?;
                        self.skip(len)?;
                    } else if item == 9 {
                        let nested = self.u32()?;
                        let inner_count = self.u64()?;
                        let width = type_width(nested);
                        if width > 0 {
                            self.skip(width * inner_count)?;
                        } else {
                            for _ in 0..inner_count {
                                let _ = self.value(nested)?;
                            }
                        }
                    } else {
                        self.skip(type_width(item))?;
                    }
                }
                Value::None
            }
            10 => Value::UInt(self.u64()?),
            11 => Value::Int(self.u64()? as i64),
            12 => Value::Float(self.f64()?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown gguf value type {other}"),
                ))
            }
        })
    }
}

fn type_width(kind: u32) -> u64 {
    match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => 0,
    }
}

fn as_u64(value: &Value) -> Option<u64> {
    match value {
        Value::UInt(v) => Some(*v),
        Value::Int(v) if *v >= 0 => Some(*v as u64),
        Value::Float(v) if *v >= 0.0 => Some(*v as u64),
        _ => None,
    }
}

fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::Text(v) => Some(v.clone()),
        _ => None,
    }
}

/// Read the header, metadata and tensor index of a `.gguf` file.
pub fn read_meta(path: &Path) -> Result<GgufMeta, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = Reader {
        inner: BufReader::with_capacity(1 << 20, file),
    };
    let magic = reader.take(4).map_err(|e| e.to_string())?;
    if magic != MAGIC {
        return Err("not a GGUF file (bad magic)".to_string());
    }
    let version = reader.u32().map_err(|e| e.to_string())?;
    if version < 2 {
        return Err(format!("unsupported GGUF version {version}"));
    }
    let tensor_count = reader.u64().map_err(|e| e.to_string())?;
    let kv_count = reader.u64().map_err(|e| e.to_string())?;

    let mut meta = GgufMeta {
        version,
        tensor_count,
        ..Default::default()
    };

    for _ in 0..kv_count {
        let key = reader.string().map_err(|e| e.to_string())?;
        let kind = reader.u32().map_err(|e| e.to_string())?;
        let value = reader.value(kind).map_err(|e| e.to_string())?;
        match key.as_str() {
            "general.architecture" => meta.arch = as_string(&value).unwrap_or_default(),
            "general.name" => meta.name = as_string(&value).unwrap_or_default(),
            "general.size_label" => meta.size_label = as_string(&value).unwrap_or_default(),
            "general.file_type" => meta.file_type = as_u64(&value).map(|v| v as u32),
            _ => {}
        }
        let suffix = key.rsplit('.').next().unwrap_or("");
        let fits_arch = key.contains('.') && key.split('.').count() >= 3;
        if fits_arch && suffix == "context_length" {
            meta.context_length = as_u64(&value).unwrap_or(0);
        } else if fits_arch && suffix == "block_count" {
            meta.block_count = as_u64(&value).unwrap_or(0);
        } else if fits_arch && suffix == "embedding_length" {
            meta.embedding_length = as_u64(&value).unwrap_or(0);
        }
    }

    // Tensor index: sum of element counts gives an exact parameter count.
    let mut params: u64 = 0;
    for _ in 0..tensor_count {
        let _name = reader.string().map_err(|e| e.to_string())?;
        let dims = reader.u32().map_err(|e| e.to_string())?;
        let mut count: u64 = 1;
        for _ in 0..dims {
            count = count.saturating_mul(reader.u64().map_err(|e| e.to_string())?);
        }
        let _kind = reader.u32().map_err(|e| e.to_string())?;
        let _offset = reader.u64().map_err(|e| e.to_string())?;
        params = params.saturating_add(count);
    }
    meta.params = params;
    Ok(meta)
}

/// llama.cpp `llama_ftype` names for the values we are likely to meet.
pub fn file_type_name(file_type: u32) -> &'static str {
    match file_type {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        33 => "Q4_0_4_4",
        34 => "Q4_0_4_8",
        35 => "Q4_0_8_8",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4",
        _ => "unknown",
    }
}

/// Guess the quantisation from a file name, e.g. `model-Q4_K_M.gguf`.
pub fn quant_from_name(name: &str) -> Option<String> {
    const KNOWN: &[&str] = &[
        "IQ1_S", "IQ1_M", "IQ2_XXS", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XXS", "IQ3_XS", "IQ3_S",
        "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S",
        "Q4_K_M", "Q4_K", "Q5_K_S", "Q5_K_M", "Q5_K", "Q6_K", "Q8_0", "Q5_0", "Q5_1", "Q4_0",
        "Q4_1", "Q3_0", "BF16", "F16", "F32", "MXFP4",
    ];
    KNOWN
        .iter()
        .find(|q| {
            name.to_ascii_uppercase().contains(&format!("-{q}"))
                || name.to_ascii_uppercase().contains(&format!("_{q}"))
        })
        .map(|q| q.to_string())
}

impl GgufMeta {
    pub fn quant_label(&self, file_name: &str) -> String {
        if let Some(ft) = self.file_type {
            let name = file_type_name(ft);
            if name != "unknown" {
                return name.to_string();
            }
        }
        quant_from_name(file_name).unwrap_or_else(|| "?".to_string())
    }

    pub fn params_label(&self) -> String {
        if self.params == 0 {
            return "?".to_string();
        }
        let p = self.params as f64;
        if p >= 1e12 {
            format!("{:.2}T", p / 1e12)
        } else if p >= 1e9 {
            format!("{:.2}B", p / 1e9)
        } else if p >= 1e6 {
            format!("{:.0}M", p / 1e6)
        } else {
            format!("{:.0}K", p / 1e3)
        }
    }
}
