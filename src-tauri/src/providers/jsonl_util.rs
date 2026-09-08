use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Result;
use crate::providers::FileCursor;

pub struct FileUpdate {
    pub new_offset: u64,
    pub size: u64,
    /// 本次是从 0 重读(文件比游标短,说明被截断/重写/轮转)。
    /// 调用方**不能**把它当增量累加,必须走全量重建。
    pub truncated: bool,
    pub lines: Vec<String>,
}

/// 从 offset 起读取文件新增内容;只消费完整行(末尾不完整的行留给下一次),文件被截断时从头重读
pub fn read_appended(path: &Path, offset: u64) -> Result<Option<FileUpdate>> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let truncated = size < offset;
    let offset = if truncated { 0 } else { offset };
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
        truncated,
        lines,
    }))
}

/// 跨扫描去重窗口:流式写盘产生的重复行常常分落在两次扫描之间,
/// 只靠单次 HashSet 会各计一次。窗口只保留最近的 N 个 key。
pub const DEDUP_WINDOW: usize = 64;

pub fn load_seen(cursor: &FileCursor) -> Vec<String> {
    cursor
        .extra
        .get("seen")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_seen(cursor: &mut FileCursor, seen: &[String]) {
    let tail = &seen[seen.len().saturating_sub(DEDUP_WINDOW)..];
    if !cursor.extra.is_object() {
        cursor.extra = serde_json::json!({});
    }
    cursor.extra["seen"] = serde_json::json!(tail);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("otr-jsonl-{name}-{suffix}.jsonl"))
    }

    fn write(path: &Path, content: &str) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn reads_only_complete_lines() {
        let p = temp_file("partial");
        write(&p, "{\"a\":1}\n{\"b\":2}\n{\"c\":");
        let up = read_appended(&p, 0).unwrap().unwrap();
        assert_eq!(up.lines.len(), 2, "末尾不完整的行必须留给下一次");
        assert!(!up.truncated);
        assert_eq!(up.new_offset, 16);
        let _ = fs::remove_file(p);
    }

    #[test]
    fn truncation_reports_flag_and_rereads_from_zero() {
        let p = temp_file("trunc");
        write(&p, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n");
        let first = read_appended(&p, 0).unwrap().unwrap();
        assert_eq!(first.lines.len(), 3);
        assert!(!first.truncated);

        // 截断重写为两行:必须报告 truncated,否则调用方会当增量累加 → 双计
        write(&p, "{\"a\":1}\n{\"b\":2}\n");
        let second = read_appended(&p, first.new_offset).unwrap().unwrap();
        assert!(second.truncated, "文件变短必须报告截断");
        assert_eq!(second.lines.len(), 2);
        assert_eq!(second.new_offset, 16);
        let _ = fs::remove_file(p);
    }

    #[test]
    fn growth_without_newline_returns_none() {
        let p = temp_file("nonewline");
        write(&p, "{\"a\":1}\n");
        let up = read_appended(&p, 0).unwrap().unwrap();
        write(&p, "{\"a\":1}\n{\"b\":");
        assert!(read_appended(&p, up.new_offset).unwrap().is_none());
        let _ = fs::remove_file(p);
    }

    #[test]
    fn dedup_window_is_bounded_and_roundtrips() {
        let mut cursor = FileCursor::default();
        let keys: Vec<String> = (0..DEDUP_WINDOW + 10).map(|i| format!("k{i}")).collect();
        save_seen(&mut cursor, &keys);
        let loaded = load_seen(&cursor);
        assert_eq!(loaded.len(), DEDUP_WINDOW, "窗口必须封顶");
        assert_eq!(
            loaded.last().unwrap(),
            &format!("k{}", DEDUP_WINDOW + 9),
            "必须保留最近的 key"
        );
    }
}
