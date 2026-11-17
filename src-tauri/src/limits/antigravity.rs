//! Antigravity 额度:问本机正在跑的语言服务。
//!
//! 语言服务把端口和 CSRF 写在
//! `~/.gemini/antigravity/daemon/hub_<httpPort>.json`。
//! 请求只打 `127.0.0.1`,不走系统代理,也不走 Tun。
//!
//! `remainingFraction` 是剩余比例。已用百分比是 `1 - remaining`。
//! 缺这个数、或窗口被标成停用,就不画那一条。

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

const SERVICE: &str = "exa.language_server_pb.LanguageServerService";
const NOT_RUNNING: &str = "Antigravity 没有在运行。打开应用后再刷新。";
const EMPTY: &str = "语言服务没有返回额度。";

struct Hub {
    pid: u32,
    port: u16,
    csrf: String,
    modified: SystemTime,
}

pub fn fetch() -> ProviderLimits {
    fetch_from_home(&gemini_home())
}

pub fn fetch_from_home(home: &Path) -> ProviderLimits {
    let hubs = live_hubs(&home.join("antigravity").join("daemon"));
    if hubs.is_empty() {
        return ProviderLimits::unconfigured("antigravity", "Antigravity", "antigravity", NOT_RUNNING);
    }

    let mut saw_live = false;
    for hub in hubs {
        if !pid_alive(hub.pid) {
            continue;
        }
        saw_live = true;
        match post_quota(hub.port, &hub.csrf) {
            Ok(body) => {
                let windows = parse_summary(&body);
                if windows.is_empty() {
                    continue;
                }
                let mut out = ProviderLimits::new("antigravity", "Antigravity", "antigravity");
                out.configured = true;
                out.windows = windows;
                return out;
            }
            Err(_) => continue,
        }
    }

    let mut out = ProviderLimits::new("antigravity", "Antigravity", "antigravity");
    if saw_live {
        out.configured = true;
        out.error = Some(EMPTY.into());
    } else {
        out.error = Some(NOT_RUNNING.into());
    }
    out
}

fn gemini_home() -> PathBuf {
    if let Ok(home) = std::env::var("GEMINI_CLI_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    crate::paths::home().join(".gemini")
}

fn live_hubs(dir: &Path) -> Vec<Hub> {
    let mut hubs = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return hubs,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("hub_") && name.ends_with(".json")) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(hub) = parse_hub(&value, modified) else {
            continue;
        };
        hubs.push(hub);
    }
    hubs.sort_by(|a, b| b.modified.cmp(&a.modified));
    hubs
}

fn parse_hub(value: &Value, modified: SystemTime) -> Option<Hub> {
    let pid = value.get("pid")?.as_u64()? as u32;
    let port = value.get("httpPort")?.as_u64()? as u16;
    let csrf = value.get("csrfToken")?.as_str()?.trim().to_string();
    if pid == 0 || port == 0 || csrf.is_empty() {
        return None;
    }
    Some(Hub {
        pid,
        port,
        csrf,
        modified,
    })
}

fn post_quota(port: u16, csrf: &str) -> Result<Value, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|_| "语言服务连不上".to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let body = "{}";
    let path = format!("/{SERVICE}/RetrieveUserQuotaSummary");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         connect-protocol-version: 1\r\n\
         x-codeium-csrf-token: {csrf}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "语言服务连不上".to_string())?;
    let (status, payload) = read_http(&mut stream)?;
    if status != 200 {
        return Err(format!("语言服务返回 {status}"));
    }
    serde_json::from_slice(&payload).map_err(|_| "语言服务返回的不是额度数据".into())
}

fn read_http(stream: &mut TcpStream) -> Result<(u16, Vec<u8>), String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if let Some(parsed) = split_http(&buf) {
            if parsed.complete {
                return Ok((parsed.status, parsed.body));
            }
        }
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                break;
            }
            Err(_) => return Err("语言服务没有返回额度".into()),
        }
    }
    split_http(&buf)
        .filter(|parsed| parsed.status > 0)
        .map(|parsed| (parsed.status, parsed.body))
        .ok_or_else(|| "语言服务没有返回额度".into())
}

struct HttpParts {
    status: u16,
    body: Vec<u8>,
    complete: bool,
}

fn split_http(buf: &[u8]) -> Option<HttpParts> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let mut lines = head.lines();
    let status_line = lines.next()?;
    let status = status_line.split_whitespace().nth(1)?.parse().ok()?;
    let mut length = None;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().ok();
        }
    }
    let body_at = head_end + 4;
    let available = buf.len().saturating_sub(body_at);
    let take = length.unwrap_or(available).min(available);
    Some(HttpParts {
        status,
        body: buf[body_at..body_at + take].to_vec(),
        complete: length.is_some_and(|len| available >= len),
    })
}

/// 把 `RetrieveUserQuotaSummary` 收成额度窗口。剩余比例换算成已用百分比。
pub fn parse_summary(body: &Value) -> Vec<QuotaWindow> {
    let groups = body
        .pointer("/response/groups")
        .and_then(Value::as_array)
        .or_else(|| body.get("groups").and_then(Value::as_array));
    let Some(groups) = groups else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for group in groups {
        let display = group
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let group_id = group_id(display);
        let buckets = group.get("buckets").and_then(Value::as_array);
        let Some(buckets) = buckets else {
            continue;
        };
        for bucket in buckets {
            if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let Some((key, label, seconds)) = window_kind(bucket.get("window").and_then(Value::as_str))
            else {
                continue;
            };
            let Some(remaining) = remaining_fraction(bucket) else {
                continue;
            };
            let used = ((1.0 - remaining) * 100.0).clamp(0.0, 100.0);
            windows.push(
                QuotaWindow::new(key, label)
                    .percent(Some(used))
                    .reset(reset_ms(bucket.get("resetTime")))
                    .window(Some(seconds))
                    .pool(&group_id, ""),
            );
        }
    }
    windows
}

fn group_id(display: &str) -> String {
    let lower = display.to_lowercase();
    if lower.contains("argon") {
        "argon".into()
    } else if lower.contains("claude") || lower.contains("gpt") {
        "claude_gpt".into()
    } else if lower.contains("flash") && !lower.contains("models") {
        "gemini_flash".into()
    } else if lower.contains("gemini") && lower.contains("pro") && !lower.contains("models") {
        "gemini_pro".into()
    } else if lower.contains("gemini") {
        "gemini".into()
    } else if display.is_empty() {
        "quota".into()
    } else {
        display
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '_' })
            .collect()
    }
}

fn window_kind(window: Option<&str>) -> Option<(&'static str, &'static str, i64)> {
    match window.unwrap_or("").trim().to_lowercase().as_str() {
        "weekly" => Some(("weekly", "每周", 7 * 24 * 3600)),
        "5h" | "5-hour" | "session" => Some(("five_hour", "5 小时", 5 * 3600)),
        _ => None,
    }
}

fn remaining_fraction(bucket: &Value) -> Option<f64> {
    let value = bucket.get("remainingFraction")?;
    let n = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    n.is_finite().then_some(n)
}

fn reset_ms(value: Option<&Value>) -> Option<i64> {
    let text = value?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE as u32
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_becomes_paired_pools() {
        let windows = parse_summary(&serde_json::json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            { "window": "weekly", "remainingFraction": 0.75, "resetTime": "2026-11-23T06:54:28Z" },
                            { "window": "5h", "remainingFraction": 1.0, "resetTime": "2026-11-16T11:54:28Z" }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            { "window": "weekly", "remainingFraction": 0.5 },
                            { "window": "5h", "remainingFraction": 0.0 }
                        ]
                    },
                    {
                        "displayName": "Gemini 4 Argon",
                        "buckets": [
                            { "window": "weekly", "remainingFraction": 0.2 },
                            { "window": "5h", "disabled": true, "remainingFraction": 0.9 }
                        ]
                    }
                ]
            }
        }));
        let key = |group: &str, window: &str| {
            windows.iter().find(|w| w.group.as_deref() == Some(group) && w.key == window)
        };
        assert!((key("gemini", "weekly").unwrap().used_percent.unwrap() - 25.0).abs() < 1e-9);
        assert_eq!(key("gemini", "five_hour").unwrap().used_percent, Some(0.0));
        assert_eq!(key("gemini", "five_hour").unwrap().window_seconds, Some(5 * 3600));
        assert!(key("gemini", "weekly").unwrap().reset_at.is_some());
        assert_eq!(key("claude_gpt", "five_hour").unwrap().used_percent, Some(100.0));
        assert!(key("argon", "weekly").is_some());
        assert!(key("argon", "five_hour").is_none());
    }

    #[test]
    fn missing_fraction_is_not_a_bar() {
        let windows = parse_summary(&serde_json::json!({
            "response": { "groups": [{ "displayName": "Gemini Models", "buckets": [{ "window": "weekly" }] }] }
        }));
        assert!(windows.is_empty());
    }
}
