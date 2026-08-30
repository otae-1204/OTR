use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Result;

pub struct FileUpdate {
    pub new_offset: u64,
    pub size: u64,
    pub lines: Vec<String>,
}

/// 从 offset 起读取文件新增内容;只消费完整行(末尾不完整的行留给下一次),文件被截断时从头重读
pub fn read_appended(path: &Path, offset: u64) -> Result<Option<FileUpdate>> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let offset = if size < offset { 0 } else { offset };
    if size <= offset {
        return Ok(None);
    }
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity((size - offset) as usize);
    f.read_to_end(&mut buf)?;
    let cut = match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => return Ok(None),
    };
    let text = String::from_utf8_lossy(&buf[..cut]).into_owned();
    let lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect();
    Ok(Some(FileUpdate {
        new_offset: offset + cut as u64,
        size,
        lines,
    }))
}

pub fn file_mtime_ms(path: &Path) -> i64 {
    use std::time::UNIX_EPOCH;
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 递归收集目录下的 *.jsonl,限制深度
pub fn collect_jsonl(dir: &Path, max_depth: u32, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if max_depth > 0 {
                collect_jsonl(&p, max_depth - 1, out);
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

/// JSON 里 u64 字段的宽容读取
pub fn u64f(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

pub fn f64f(v: &serde_json::Value, key: &str) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

pub fn strstr<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}
