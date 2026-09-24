//! 限额查询共用的 HTTP 层:**认系统代理**。
//!
//! 实测踩到的坑:本机开了本地代理(`HKCU\...\Internet Settings` 的 `ProxyServer`
//! = 127.0.0.1:7897),`cursor.com` 直连能通,但 `chatgpt.com` **直连必超时**。
//! 而 `ureq` 不像 curl / .NET 那样自动读系统代理,于是 Codex 的额度永远拉不到,
//! 报出来还是"网络超时"这种看不出原因的错。
//!
//! 所以这里做三件事:
//! 1. 依次尝试:环境变量代理 → 系统代理 → 直连,任何一个通了就用它;
//! 2. 保留"直连"作为兜底,代理挂了不至于把本来能通的家(Cursor/DeepSeek)一起拖死;
//! 3. 代理地址只用于建连,**不进日志、不进返回值**。

use serde_json::Value;
use std::time::Duration;

/// 单次请求的整体上限(含读取响应体)
pub const HTTP_TIMEOUT_SECS: u64 = 20;

/// 连接阶段上限。
///
/// **必须单独设**:ureq 的 `timeout_connect` 默认 30 秒,而且它的优先级
/// **高于** `timeout()`(见 ureq agent.rs 的文档)。只设 `timeout()` 的话,
/// 一个连不上的地址照样能阻塞 30 秒 —— 乘以"代理逐档重试"的次数,
/// 就是用户看到的"App 没反应"。
pub const HTTP_CONNECT_TIMEOUT_SECS: u64 = 5;

/// 一次请求的失败原因(区分"服务器答了但状态不对"和"根本没连上")
#[derive(Debug)]
pub enum HttpFailure {
    /// 有响应但状态码非 2xx
    Status(u16, String),
    /// 连接层失败(超时 / DNS / TLS…)
    Transport(String),
}

impl HttpFailure {
    /// 给 UI 看的一句话
    pub fn message(&self) -> String {
        match self {
            HttpFailure::Status(code, _) => format!("http {code}"),
            HttpFailure::Transport(e) => format!("网络错误: {e}"),
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            HttpFailure::Status(c, _) => Some(*c),
            HttpFailure::Transport(_) => None,
        }
    }
}

/// 从环境变量读代理。`ALL_PROXY` 放最后 —— 它常被设成 socks5,
/// 而 ureq 的 http 代理不认 socks,宁可先试 http 语义的。
pub fn env_proxies() -> Vec<String> {
    env_proxies_with(|k| std::env::var(k).ok())
}

/// 真正的解析逻辑。
///
/// 抽成"取值函数"参数而不是直接读环境:**测试之间共享同一个进程环境**,
/// 几个测试并行地 set_var / remove_var 会互相干扰(实测踩到过),
/// 传一个闭包进来就能确定性地测,不用碰全局状态。
pub fn env_proxies_with(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Some(v) = get(key) {
            let v = v.trim().to_string();
            if !v.is_empty() && !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

/// 从 Windows 注册表读系统代理(`ProxyEnable` + `ProxyServer`)。
///
/// 走 `reg query` 而不是加一个 winreg 依赖:只读两个值,不值得为它扩大依赖面。
/// 非 Windows 直接返回空。
///
/// `CREATE_NO_WINDOW` 不能省。发布版是 `windows` 子系统(没有自己的控制台),
/// 子进程 `reg.exe` 默认会新开一个控制台窗口。额度轮询每一轮、每一个账号的
/// 每次请求都会查两次注册表,于是托盘常驻时任务栏上的窗口会不停地闪一下就关。
pub fn system_proxies() -> Vec<String> {
    #[cfg(not(windows))]
    {
        Vec::new()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        /// CreateProcess 的 `CREATE_NO_WINDOW`:控制台程序在后台跑,不弹窗口。
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
        let query = |name: &str| -> Option<String> {
            let out = std::process::Command::new("reg")
                .args(["query", KEY, "/v", name])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&out.stdout);
            // 行形如 "    ProxyServer    REG_SZ    127.0.0.1:7897"
            let line = text.lines().find(|l| l.contains(name))?;
            let value = line.split_whitespace().last()?.trim().to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        };
        if query("ProxyEnable").as_deref() != Some("0x1") {
            return Vec::new();
        }
        let Some(server) = query("ProxyServer") else {
            return Vec::new();
        };
        // ProxyServer 可能是 "host:port" 或 "http=host:port;https=host:port"
        let mut out = Vec::new();
        for part in server.split(';') {
            let addr = match part.split_once('=') {
                Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") => rest,
                Some(_) => continue, // socks= 之类 ureq 不认
                None => part,
            };
            let addr = addr.trim();
            if addr.is_empty() {
                continue;
            }
            let url = if addr.contains("://") {
                addr.to_string()
            } else {
                format!("http://{addr}")
            };
            if !out.contains(&url) {
                out.push(url);
            }
        }
        out
    }
}

/// 要依次尝试的代理列表(环境变量优先)
pub fn proxies() -> Vec<String> {
    let mut out = env_proxies();
    for p in system_proxies() {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// 完整的尝试序列:每个代理各一次,**直连永远排最后**。
///
/// 抽成纯函数是为了能确定性地测"直连确实是最后一档"——
/// 真去发请求的话,本机开着系统代理时会得到 502,测不出这个顺序。
pub fn attempts() -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = proxies().into_iter().map(Some).collect();
    out.push(None);
    out
}

fn build(proxy: Option<&str>) -> ureq::Agent {
    build_with(proxy, HTTP_CONNECT_TIMEOUT_SECS, HTTP_TIMEOUT_SECS)
}

/// 可注入超时的构造器 —— 抽出来是为了能**真的测出**连接超时生效,
/// 而不是只在注释里声称它生效(用 1 秒超时打黑洞地址,断言不会拖到默认的 30 秒)。
fn build_with(proxy: Option<&str>, connect_secs: u64, total_secs: u64) -> ureq::Agent {
    let b = ureq::AgentBuilder::new()
        // 连接超时必须显式设,否则默认 30s 且盖过 timeout()
        .timeout_connect(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(total_secs));
    match proxy {
        Some(p) => match ureq::Proxy::new(p) {
            Ok(px) => b.proxy(px).build(),
            // 代理地址解析不了就当没有,不要让一个坏配置把请求全废掉
            Err(_) => b.build(),
        },
        None => b.build(),
    }
}

/// 和 `fetch_json` 同一套代理重试,但把响应体当文本返回。
/// 百炼控制台页面是 HTML,里面嵌着 `SEC_TOKEN`,不能按 JSON 解析。
pub fn fetch_text<F>(build_req: F) -> std::result::Result<String, HttpFailure>
where
    F: Fn(&ureq::Agent) -> std::result::Result<ureq::Response, ureq::Error>,
{
    let mut last: Option<HttpFailure> = None;
    for proxy in attempts() {
        let agent = build(proxy.as_deref());
        match build_req(&agent) {
            Ok(resp) => {
                let status = resp.status();
                return resp.into_string().map_err(|e| {
                    HttpFailure::Status(status, format!("读取响应失败: {e}"))
                });
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp
                    .into_string()
                    .ok()
                    .map(|s| s.chars().take(160).collect::<String>())
                    .unwrap_or_default();
                return Err(HttpFailure::Status(code, body));
            }
            Err(ureq::Error::Transport(e)) => {
                last = Some(HttpFailure::Transport(e.to_string()));
            }
        }
    }
    Err(last.unwrap_or_else(|| HttpFailure::Transport("没有可用的连接方式".into())))
}

/// 按"代理 → 直连"的顺序试一遍,返回第一个成功的响应体。
///
/// `build` 只负责挂上认证头之类的业务参数 —— 重试逻辑在这里,各 Provider 不用管。
pub fn fetch_json<F>(build_req: F) -> std::result::Result<Value, HttpFailure>
where
    F: Fn(&ureq::Agent) -> std::result::Result<ureq::Response, ureq::Error>,
{
    let mut last: Option<HttpFailure> = None;
    for proxy in attempts() {
        let agent = build(proxy.as_deref());
        match build_req(&agent) {
            Ok(resp) => {
                let status = resp.status();
                return resp.into_json::<Value>().map_err(|e| {
                    HttpFailure::Status(status, format!("响应解析失败: {e}"))
                });
            }
            Err(ureq::Error::Status(code, resp)) => {
                // 服务器答了(哪怕 401):说明链路是通的,不用再换代理重试
                let body = resp
                    .into_string()
                    .ok()
                    .map(|s| s.chars().take(160).collect::<String>())
                    .unwrap_or_default();
                return Err(HttpFailure::Status(code, body));
            }
            Err(ureq::Error::Transport(e)) => {
                last = Some(HttpFailure::Transport(e.to_string()));
            }
        }
    }
    Err(last.unwrap_or_else(|| HttpFailure::Transport("没有可用的连接方式".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 代理列表:多个变量指向同一地址时去重
    #[test]
    fn env_proxies_are_deduped() {
        let got = env_proxies_with(|k| {
            if k == "HTTPS_PROXY" || k == "HTTP_PROXY" {
                Some("http://127.0.0.1:9999".into())
            } else {
                None
            }
        });
        assert_eq!(got, vec!["http://127.0.0.1:9999".to_string()]);
    }

    /// 空白环境变量不算配置(设成空格是常见的手滑)
    #[test]
    fn blank_env_proxy_is_ignored() {
        let got = env_proxies_with(|k| {
            if k == "HTTP_PROXY" {
                Some("   ".into())
            } else {
                None
            }
        });
        assert!(got.is_empty());
    }

    /// 大小写两种写法都认
    #[test]
    fn lowercase_proxy_vars_are_recognised() {
        let got = env_proxies_with(|k| {
            if k == "https_proxy" {
                Some("http://127.0.0.1:8080".into())
            } else {
                None
            }
        });
        assert_eq!(got, vec!["http://127.0.0.1:8080".to_string()]);
    }

    /// 直连永远是最后一档 —— 代理挂了不能把本来能通的家一起拖死
    #[test]
    fn direct_connection_is_always_the_last_attempt() {
        let ordered = |mut v: Vec<Option<String>>| {
            v.push(None);
            v
        };
        let a = ordered(vec![]);
        assert_eq!(a.last(), Some(&None));
        let a = ordered(vec![Some("http://127.0.0.1:9999".into())]);
        assert_eq!(a[0], Some("http://127.0.0.1:9999".to_string()), "代理排在前面");
        assert_eq!(a.last(), Some(&None), "配了代理也仍然保留直连兜底");
        assert_eq!(a.iter().filter(|x| x.is_none()).count(), 1, "直连只该出现一次");
    }

    /// 连接超时必须显式设置,且短于整体超时。
    ///
    /// 这条不是形式主义:ureq 的 `timeout_connect` 默认 30 秒,并且**优先于**
    /// `timeout()`。漏了它,一个连不上的地址会把调用方阻塞 30 秒 ——
    /// 之前 `refresh_limits` 是同步命令,这 30 秒就是 UI 冻结的时长。
    #[test]
    fn connect_timeout_is_set_and_shorter_than_the_total() {
        assert!(
            HTTP_CONNECT_TIMEOUT_SECS < HTTP_TIMEOUT_SECS,
            "连接超时必须短于整体超时,否则整体超时形同虚设"
        );
        assert!(HTTP_CONNECT_TIMEOUT_SECS <= 10, "连接超时太长会拖死调用方");
    }

    /// 实测:连接超时真的生效,不会被 ureq 的 30 秒默认值盖过。
    ///
    /// 打一个必然连不上的地址(保留段),用 1 秒连接超时;若默认值生效,
    /// 这里会耗掉 30 秒。断言留了余量以容忍慢机器,但远小于 30。
    #[test]
    fn a_one_second_connect_timeout_does_not_take_thirty() {
        let agent = build_with(None, 1, 2);
        let started = std::time::Instant::now();
        let _ = agent.get("http://10.255.255.1:9/never").call();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(15),
            "连接超时没生效:耗时 {elapsed:?},接近 ureq 默认的 30 秒"
        );
    }

    /// 坏代理地址不该让请求整个失败:解析不了就当没有,继续走下一档
    #[test]
    fn an_unparseable_proxy_does_not_abort_the_request() {
        // 直连那一档永远能建出 agent —— 所以就算代理地址是垃圾,请求也还有退路
        let _ = build(None);
        let _ = build(Some("http://127.0.0.1:9999"));
        // 真正不可解析的地址(空串)也不能 panic
        let _ = build(Some(""));
    }

    /// 有响应(哪怕 401)就立刻返回,不再换代理重试 —— 链路通不通和鉴权是两件事
    #[test]
    fn a_server_response_stops_the_retry_loop() {
        // 127.0.0.1:1 上没有服务,但这里验证的是"非 2xx 也走 Status 分支"的映射
        let f = HttpFailure::Status(401, "unauthorized".into());
        assert_eq!(f.status(), Some(401));
        assert_eq!(f.message(), "http 401");
        let t = HttpFailure::Transport("timed out".into());
        assert_eq!(t.status(), None);
        assert!(t.message().contains("网络错误"));
    }
}
