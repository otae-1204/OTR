//! 登录窗自动取控制台 Cookie。
//!
//! 只在用户点「自动获取」时开窗。Cookie 读到合格形状后写进该账号已有的凭据条目,
//! 命令只返回一句状态,不返回 Cookie、JWT 或票据。
//!
//! Windows 上 `cookies()` 不能在事件循环线程里调用,读取放在独立线程。
//! 每个来源用自己的数据目录,不碰用户日常浏览器,也不碰 OTR 主窗口的 profile。

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::credential;

const WAIT: Duration = Duration::from_secs(10 * 60);

pub fn capture(app: &AppHandle, provider: &str, credential_name: &str) -> Result<String, String> {
    if !credential::is_allowed_name(credential_name) {
        return Err(format!("未知的凭据条目:{credential_name}"));
    }
    let target = Target::from_provider(provider)?;
    let label = format!("login-{provider}");
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.destroy();
    }

    let dir = std::env::temp_dir().join(format!("otr-cookie-login-{provider}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建登录目录失败:{e}"))?;

    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(target.start.parse().map_err(|e| format!("登录地址无效:{e}"))?))
        .title(target.title)
        .inner_size(1100.0, 760.0)
        .center()
        .focused(true)
        .data_directory(dir)
        .on_navigation(|url| url.scheme() == "https" || url.scheme() == "http")
        .build()
        .map_err(|e| format!("打不开登录窗口:{e}"))?;

    let started = Instant::now();
    let mut failure = "登录窗口已关闭,没有取到凭据".to_string();
    loop {
        if started.elapsed() > WAIT {
            let _ = win.destroy();
            return Err("等待登录超时,已有凭据没有改动".into());
        }
        if app.get_webview_window(&label).is_none() {
            return Err(failure);
        }
        std::thread::sleep(Duration::from_secs(2));
        let Some(current) = app.get_webview_window(&label) else {
            return Err(failure);
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(current.cookies());
        });
        let cookies = match rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(list)) => list,
            Ok(Err(_)) => continue,
            Err(_) => continue,
        };
        match target.secret(&cookies) {
            Ok(secret) => {
                credential::set(credential_name, &secret)?;
                let _ = win.destroy();
                return Ok(target.saved);
            }
            Err(err) => failure = err,
        }
    }
}

struct Target {
    title: &'static str,
    start: &'static str,
    saved: String,
    secret: fn(&[tauri::webview::Cookie<'static>]) -> Result<String, String>,
}

impl Target {
    fn from_provider(provider: &str) -> Result<Self, String> {
        match provider {
            "stepfun" => Ok(Self {
                title: "登录 StepFun",
                start: "https://platform.stepfun.com",
                saved: "已获取 StepFun 订阅凭据".into(),
                secret: stepfun_secret,
            }),
            "qwen" => Ok(Self {
                title: "登录阿里云百炼",
                start: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
                saved: "已获取百炼控制台 Cookie".into(),
                secret: qwen_secret,
            }),
            _ => Err("这个来源不需要登录获取".into()),
        }
    }

    fn secret(&self, cookies: &[tauri::webview::Cookie<'static>]) -> Result<String, String> {
        (self.secret)(cookies)
    }
}

fn stepfun_secret(cookies: &[tauri::webview::Cookie<'static>]) -> Result<String, String> {
    let Some(cookie) = cookies.iter().find(|c| c.name().eq_ignore_ascii_case("Oasis-Token")) else {
        return Err("还没有登录。登录完成后这个窗口会自己关掉。".into());
    };
    let journal = super::stepfun::extract_journal(cookie.value())
        .ok_or_else(|| "登录态格式不对,没有取到订阅凭据。".to_string())?;
    Ok(format!("Oasis-Token={journal}"))
}

fn qwen_secret(cookies: &[tauri::webview::Cookie<'static>]) -> Result<String, String> {
    let mut pairs = Vec::new();
    let mut ticket = false;
    for cookie in cookies {
        let name = cookie.name();
        if !qwen_cookie_name(name) {
            continue;
        }
        let value = cookie.value();
        if value.is_empty() {
            continue;
        }
        if name == "login_aliyunid_ticket" {
            ticket = true;
        }
        pairs.push(format!("{name}={value}"));
    }
    if !ticket {
        return Err("还没有登录。登录完成后这个窗口会自己关掉。".into());
    }
    Ok(pairs.join("; "))
}

/// 百炼额度实际会读到的名字,再加上登录态本身。其余跟踪 Cookie 不存。
fn qwen_cookie_name(name: &str) -> bool {
    name == "login_aliyunid_ticket"
        || name == "login_aliyunid_csrf"
        || name == "login_aliyunid"
        || name == "login_aliyunid_pk"
        || name == "login_aliyunid_pks"
        || name == "cna"
        || name.contains("sec_token")
        || name.contains("csrf")
        || name.contains("x5sec")
}

#[cfg(test)]
mod tests {
    use super::{qwen_secret, stepfun_secret};
    use tauri::webview::Cookie;

    fn cookie(name: &str, value: &str) -> Cookie<'static> {
        Cookie::from((name.to_string(), value.to_string()))
    }

    #[test]
    fn stepfun_waits_until_the_journal_shape_appears() {
        let missing = stepfun_secret(&[cookie("_wafdytokenv1", "abc")]);
        assert!(missing.is_err());
        let jwt = "eyJhbGciOiJub25lIn0.eyJkZXZpY2VfaWQiOiJkZXYxIn0.sig";
        let journal = format!("{jwt}...{jwt}");
        let got = stepfun_secret(&[cookie("Oasis-Token", &journal)]).unwrap();
        assert_eq!(got, format!("Oasis-Token={journal}"));
        assert!(!got.contains(jwt.split('.').next().unwrap()) || got.starts_with("Oasis-Token="));
    }

    #[test]
    fn qwen_requires_the_login_ticket_and_keeps_only_named_cookies() {
        let early = qwen_secret(&[cookie("cna", "abc"), cookie("login_aliyunid_csrf", "csrf")]);
        assert!(early.is_err(), "没有登录票不能保存");
        let got = qwen_secret(&[
            cookie("cna", "abc"),
            cookie("login_aliyunid_ticket", "ticket"),
            cookie("isg", "tracker"),
        ])
        .unwrap();
        assert!(got.contains("cna=abc"));
        assert!(got.contains("login_aliyunid_ticket=ticket"));
        assert!(!got.contains("isg"), "跟踪 Cookie 不进凭据");
    }
}
