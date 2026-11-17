//! Antigravity 的 Token 消耗:直接读本机会话库,不走语言服务,也不跑 tokscale。
//!
//! 字段对照 tokscale 的 `antigravity_cli.rs`(MIT)。IDE 和 CLI 是同一套 SQLite:
//! `~/.gemini/antigravity/conversations/*.db` 与
//! `~/.gemini/antigravity-cli/conversations/*.db`。
//! 每一代写在 `gen_metadata.data` 的 protobuf 里。`chatModel.#4` 是用量:
//! `#2` 输入、`#3` 输出总量、`#4` 缓存写、`#5` 缓存读、`#9` 思考、`#10` 正文、
//! `#11` 响应 id。输出总量已经含思考,思考只单列,不再加进合计。
//! 时间优先用 `chatModel.#9.#4`,没有再用 `steps` 里同一响应 id 的时间,
//! 最后才退回会话创建时间。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::error::Result;
use crate::model::UsageRecord;
use crate::paths;
use crate::providers::jsonl_util;
use crate::providers::{AgentProvider, FileCursor, ScanCtx};

pub struct AntigravityProvider;

const AGENT: &str = "antigravity";

impl AgentProvider for AntigravityProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Antigravity"
    }

    fn detect(&self) -> bool {
        paths::antigravity_conversation_dirs()
            .iter()
            .any(|dir| dir.is_dir())
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        paths::antigravity_conversation_dirs()
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let mut files = Vec::new();
        for dir in paths::antigravity_conversation_dirs() {
            collect_dbs(&dir, &mut files);
        }
        let mut records = Vec::new();
        for file in files {
            if let Err(err) = scan_db(&file, ctx, &mut records) {
                eprintln!("[antigravity] {}: {err}", file.display());
            }
        }
        Ok(records)
    }
}

fn collect_dbs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("db") {
            out.push(path);
        }
    }
}

fn scan_db(path: &Path, ctx: &mut ScanCtx, out: &mut Vec<UsageRecord>) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let stamp = file_stamp(path);
    let previous_rows = if ctx.full { 0 } else { cursor.offset as usize };
    if !ctx.full && previous_rows > 0 && stamp_matches(&cursor, &stamp) {
        return Ok(());
    }

    let Some(conn) = open_ro(path) else {
        return Ok(());
    };
    let blobs = {
        let mut query = match conn.prepare("SELECT data FROM gen_metadata ORDER BY idx") {
            Ok(query) => query,
            Err(_) => return Ok(()),
        };
        let rows = match query.query_map([], |row| row.get::<_, Vec<u8>>(0)) {
            Ok(rows) => rows,
            Err(_) => return Ok(()),
        };
        let mut blobs = Vec::new();
        for row in rows {
            blobs.push(row?);
        }
        blobs
    };

    if !ctx.full && previous_rows > blobs.len() {
        ctx.force_full = true;
        return Ok(());
    }

    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let created = trajectory_created_ms(&conn);
    let project = trajectory_project(&conn);
    let steps = step_timestamps(&conn);
    let fallback_ts = created.unwrap_or_else(|| jsonl_util::file_mtime_ms(path));
    let mut seen = HashSet::new();
    let start = if ctx.full { 0 } else { previous_rows.min(blobs.len()) };
    for blob in blobs.iter().skip(start) {
        if let Some(record) = generation_record(
            blob,
            &session_id,
            project.as_deref(),
            fallback_ts,
            &steps,
            &mut seen,
        ) {
            out.push(record);
        }
    }

    cursor.offset = blobs.len() as u64;
    cursor.size = stamp.db_size;
    cursor.mtime_ms = stamp.db_mtime;
    cursor.extra = serde_json::json!({
        "walSize": stamp.wal_size,
        "walMtime": stamp.wal_mtime,
    });
    ctx.cursors.insert(key, cursor);
    Ok(())
}

struct Stamp {
    db_size: u64,
    db_mtime: i64,
    wal_size: u64,
    wal_mtime: i64,
}

fn file_stamp(path: &Path) -> Stamp {
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    Stamp {
        db_size: fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        db_mtime: jsonl_util::file_mtime_ms(path),
        wal_size: fs::metadata(&wal).map(|meta| meta.len()).unwrap_or(0),
        wal_mtime: jsonl_util::file_mtime_ms(&wal),
    }
}

fn stamp_matches(cursor: &FileCursor, stamp: &Stamp) -> bool {
    cursor.size == stamp.db_size
        && cursor.mtime_ms == stamp.db_mtime
        && cursor.extra.get("walSize").and_then(|v| v.as_u64()) == Some(stamp.wal_size)
        && cursor.extra.get("walMtime").and_then(|v| v.as_i64()) == Some(stamp.wal_mtime)
}

fn open_ro(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

fn trajectory_blob(conn: &Connection) -> Option<Vec<u8>> {
    conn.query_row(
        "SELECT data FROM trajectory_metadata_blob LIMIT 1",
        [],
        |row| row.get(0),
    )
    .ok()
}

fn trajectory_created_ms(conn: &Connection) -> Option<i64> {
    let blob = trajectory_blob(conn)?;
    message_field(&blob, 2).and_then(proto_timestamp_ms)
}

fn trajectory_project(conn: &Connection) -> Option<String> {
    let blob = trajectory_blob(conn)?;
    let uri = message_field(&blob, 1).and_then(|folder| string_field(folder, 1))?;
    project_name(uri)
}

fn project_name(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path = if let Some(stripped) = rest.strip_prefix('/') {
        if stripped.len() >= 2 && stripped.as_bytes().get(1) == Some(&b':') {
            stripped
        } else {
            rest
        }
    } else {
        rest
    };
    let name = path.rsplit(['/', '\\']).find(|part| !part.is_empty())?;
    let name = percent_decode(name);
    (!name.is_empty()).then_some(name)
}

struct StepTimes {
    by_response: Vec<(String, i64)>,
}

fn step_timestamps(conn: &Connection) -> StepTimes {
    let mut by_response = Vec::new();
    let Ok(mut query) = conn.prepare(
        "SELECT metadata FROM steps WHERE step_type = 15 AND metadata IS NOT NULL",
    ) else {
        return StepTimes { by_response };
    };
    let Ok(rows) = query.query_map([], |row| row.get::<_, Vec<u8>>(0)) else {
        return StepTimes { by_response };
    };
    for row in rows.flatten() {
        let Some(ms) = message_field(&row, 1).and_then(proto_timestamp_ms) else {
            continue;
        };
        if ms <= 0 {
            continue;
        }
        if let Some(id) = message_field(&row, 9).and_then(|meta| string_field(meta, 11)) {
            let id = id.trim();
            if !id.is_empty() {
                by_response.push((id.to_string(), ms));
            }
        }
    }
    StepTimes { by_response }
}

fn generation_record(
    blob: &[u8],
    session_id: &str,
    project: Option<&str>,
    fallback_ts: i64,
    steps: &StepTimes,
    seen: &mut HashSet<String>,
) -> Option<UsageRecord> {
    let chat = message_field(blob, 1)?;
    let usage = message_field(chat, 4)?;
    let input = varint_u64(usage, 2);
    let total_output = varint_u64(usage, 3);
    let cache_write = varint_u64(usage, 4);
    let cache_read = varint_u64(usage, 5);
    let reasoning = varint_u64(usage, 9);
    let text_output = varint_u64(usage, 10);
    let output = if total_output > 0 {
        total_output
    } else {
        text_output.saturating_add(reasoning)
    };
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }
    if let Some(id) = string_field(usage, 11).map(str::trim).filter(|id| !id.is_empty()) {
        if !seen.insert(id.to_string()) {
            return None;
        }
    }
    let response_id = string_field(usage, 11).map(str::trim).filter(|id| !id.is_empty());
    let step_ts = response_id.and_then(|id| {
        steps
            .by_response
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, ms)| *ms)
    });
    let ts = message_field(chat, 9)
        .and_then(|gen| message_field(gen, 4))
        .and_then(proto_timestamp_ms)
        .filter(|ms| *ms > 0)
        .or(step_ts)
        .filter(|ms| *ms > 0)
        .unwrap_or(fallback_ts);
    let model = string_field(chat, 19)
        .map(str::trim)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("gemini-default"))
        .or_else(|| {
            string_field(chat, 21)
                .map(str::trim)
                .filter(|label| !label.is_empty())
        })
        .unwrap_or("unknown")
        .to_string();
    Some(UsageRecord {
        agent: AGENT.into(),
        session_id: Some(session_id.to_string()),
        project: project.map(str::to_string),
        model: Some(model),
        ts,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        calls: 1,
        ..Default::default()
    })
}

fn varint_u64(buf: &[u8], field: u64) -> u64 {
    varint_field(buf, field).unwrap_or(0)
}

struct ProtoReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

enum Wire<'a> {
    Varint(u64),
    Len(&'a [u8]),
    Skip,
}

impl<'a> ProtoReader<'a> {
    fn next(&mut self) -> Option<(u64, Wire<'a>)> {
        let key = self.read_varint()?;
        let field = key >> 3;
        let wire = match key & 7 {
            0 => Wire::Varint(self.read_varint()?),
            1 => {
                self.pos = self.pos.checked_add(8).filter(|pos| *pos <= self.buf.len())?;
                Wire::Skip
            }
            2 => {
                let len = self.read_varint()? as usize;
                let end = self.pos.checked_add(len).filter(|pos| *pos <= self.buf.len())?;
                let bytes = &self.buf[self.pos..end];
                self.pos = end;
                Wire::Len(bytes)
            }
            5 => {
                self.pos = self.pos.checked_add(4).filter(|pos| *pos <= self.buf.len())?;
                Wire::Skip
            }
            _ => return None,
        };
        Some((field, wire))
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            if shift >= 64 || self.pos >= self.buf.len() {
                return None;
            }
            let byte = self.buf[self.pos];
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
        }
    }
}

fn message_field(buf: &[u8], field: u64) -> Option<&[u8]> {
    let mut reader = ProtoReader { buf, pos: 0 };
    while let Some((found, wire)) = reader.next() {
        if found == field {
            if let Wire::Len(bytes) = wire {
                return Some(bytes);
            }
        }
    }
    None
}

fn varint_field(buf: &[u8], field: u64) -> Option<u64> {
    let mut reader = ProtoReader { buf, pos: 0 };
    while let Some((found, wire)) = reader.next() {
        if found == field {
            if let Wire::Varint(value) = wire {
                return Some(value);
            }
        }
    }
    None
}

fn string_field(buf: &[u8], field: u64) -> Option<&str> {
    message_field(buf, field).and_then(|bytes| std::str::from_utf8(bytes).ok())
}

fn proto_timestamp_ms(buf: &[u8]) -> Option<i64> {
    let seconds = i64::try_from(varint_field(buf, 1)?).ok()?;
    let nanos = i64::try_from(varint_field(buf, 2).unwrap_or(0)).unwrap_or(0);
    if !(0..=999_999_999).contains(&nanos) {
        return None;
    }
    seconds.checked_mul(1000)?.checked_add(nanos / 1_000_000)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push((hi << 4) | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn enc_varint(field: u64, value: u64) -> Vec<u8> {
    let mut out = encode_varint((field << 3) | 0);
    out.extend(encode_varint(value));
    out
}

fn enc_len(field: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = encode_varint((field << 3) | 2);
    out.extend(encode_varint(payload.len() as u64));
    out.extend(payload);
    out
}

fn enc_string(field: u64, text: &str) -> Vec<u8> {
    enc_len(field, text.as_bytes())
}

fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_blob() -> Vec<u8> {
        let mut usage = enc_varint(2, 100);
        usage.extend(enc_varint(3, 50));
        usage.extend(enc_varint(5, 20));
        usage.extend(enc_varint(9, 15));
        usage.extend(enc_string(11, "resp-1"));
        let mut stamp = enc_varint(1, 1_711_200_000);
        stamp.extend(enc_varint(2, 0));
        let when = enc_len(4, &stamp);
        let mut chat = enc_len(4, &usage);
        chat.extend(enc_len(9, &when));
        chat.extend(enc_string(19, "gemini-3-flash"));
        enc_len(1, &chat)
    }

    #[test]
    fn usage_fields_match_tokscale_map() {
        let mut seen = HashSet::new();
        let record = generation_record(
            &sample_blob(),
            "sess",
            Some("Token-Show"),
            0,
            &StepTimes { by_response: Vec::new() },
            &mut seen,
        )
        .expect("row");
        assert_eq!(record.input_tokens, 100);
        assert_eq!(record.output_tokens, 50);
        assert_eq!(record.cache_read_tokens, 20);
        assert_eq!(record.reasoning_tokens, 15);
        assert_eq!(record.model.as_deref(), Some("gemini-3-flash"));
        assert_eq!(record.ts, 1_711_200_000_000);
        assert_eq!(record.calls, 1);
        assert_eq!(record.project.as_deref(), Some("Token-Show"));
    }

    #[test]
    fn duplicate_response_id_is_one_call() {
        let blob = sample_blob();
        let mut seen = HashSet::new();
        let steps = StepTimes { by_response: Vec::new() };
        assert!(generation_record(&blob, "sess", None, 1, &steps, &mut seen).is_some());
        assert!(generation_record(&blob, "sess", None, 1, &steps, &mut seen).is_none());
    }

    #[test]
    fn project_name_uses_the_directory() {
        assert_eq!(
            project_name("file:///C:/Code/Token-Show").as_deref(),
            Some("Token-Show")
        );
    }
}
