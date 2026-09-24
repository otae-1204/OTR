//! 凭据存储:优先系统凭据库(keyring → Windows 凭据管理器 / DPAPI),回退环境变量。
//!
//! **命令永不返回密钥值**:对外只暴露"是否存在"(见 `list_credentials`)。
//! 密钥一旦能从某个 Tauri 命令读出来,它就会出现在前端内存、开发者工具和任何
//! 日志里 —— 这是额度页唯一必须守住的边界。
//!
//! Windows 凭据 blob 上限是 2560 **字节**。密码按 UTF-16 存,ASCII 的 Cookie
//! 大约 1280 个字符就写不进去(百炼控制台 Cookie 经常更长)。超长的拆成多条
//! `名字`、`名字#1`、`名字#2`…,读的时候再拼回去。短密钥仍然只占原来那一条。
//!
//! 非 Windows 不引入 keyring(Linux 会拖进整套 zbus 依赖树),走"请用环境变量"分支。

/// keyring 里的 service 名
pub const SERVICE: &str = "com.otae.radar";

/// 内置账号的凭据条目 → 对应的环境变量名(解析顺序:keyring → 环境变量)。
///
/// 额外账号不进这张表。它们的条目名由程序生成,形如
/// `limit.<provider>.<id>.key` / `limit.<provider>.<id>.cookie`。
pub const KNOWN: &[(&str, &str)] = &[
    ("DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY"),
    ("QWEN_CODING_PLAN_API_KEY", "QWEN_CODING_PLAN_API_KEY"),
    ("QWEN_CONSOLE_COOKIE", "QWEN_CONSOLE_COOKIE"),
    ("STEPFUN_API_KEY", "STEPFUN_API_KEY"),
    ("STEPFUN_CONSOLE_COOKIE", "STEPFUN_CONSOLE_COOKIE"),
    ("STEPFUN_CONSOLE_TOKEN", "STEPFUN_CONSOLE_TOKEN"),
];

/// 条目名是否允许写入:内置条目,或符合生成规则的 `limit.*`。
///
/// 空串、`../`、随意起的 `DEEPSEEK_API_KEY_WORK` 都不行。生成名多出来的
/// 自由度只留在账号 id 那一段,避免命令能往凭据库写任意目标。
pub fn is_allowed_name(name: &str) -> bool {
    if KNOWN.iter().any(|(n, _)| *n == name) {
        return true;
    }
    is_generated_name(name)
}

/// `limit.<provider>.<id>.key|cookie`,provider 必须是额度来源,id 只含字母数字和横线。
pub fn is_generated_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let Some("limit") = parts.next() else {
        return false;
    };
    let Some(provider) = parts.next() else {
        return false;
    };
    let Some(id) = parts.next() else {
        return false;
    };
    let Some(kind) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if !crate::settings::LIMIT_PROVIDERS.contains(&provider) {
        return false;
    }
    if kind != "key" && kind != "cookie" {
        return false;
    }
    is_id_token(id)
}

fn is_id_token(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
        && id.len() <= 64
}

pub fn generated_key_name(provider: &str, id: &str) -> std::result::Result<String, String> {
    generated_name(provider, id, "key")
}

pub fn generated_cookie_name(provider: &str, id: &str) -> std::result::Result<String, String> {
    generated_name(provider, id, "cookie")
}

fn generated_name(provider: &str, id: &str, kind: &str) -> std::result::Result<String, String> {
    let name = format!("limit.{provider}.{id}.{kind}");
    if !is_generated_name(&name) {
        return Err(format!("账号 id 不合法:{id}"));
    }
    Ok(name)
}

/// 单条凭据的 UTF-16 码元上限。blob 是 2560 字节,UTF-16 每码元 2 字节,
/// 留一点余量,不把一条写到贴着天花板。
const CHUNK_UTF16_UNITS: usize = 1200;
/// 防止坏数据把读取循环撑爆。百炼 Cookie 拆开也就几条。
const MAX_CHUNKS: usize = 32;

/// 按 UTF-16 码元切开,且不把一个字符劈成两半。
fn split_secret(secret: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut units = 0usize;
    for ch in secret.chars() {
        let width = ch.len_utf16();
        if units + width > CHUNK_UTF16_UNITS && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
        current.push(ch);
        units += width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 第 0 片用原名,后面的片子挂 `#1`、`#2`,避免和已有的 `limit.*` 条目撞名。
fn part_name(name: &str, index: usize) -> String {
    if index == 0 {
        name.to_string()
    } else {
        format!("{name}#{index}")
    }
}

#[cfg(windows)]
mod store {
    use super::{part_name, split_secret, MAX_CHUNKS, SERVICE};

    fn entry(name: &str) -> std::result::Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, name).map_err(|e| format!("凭据库不可用: {e}"))
    }

    fn write_one(name: &str, secret: &str) -> std::result::Result<(), String> {
        entry(name)?
            .set_password(secret)
            .map_err(|e| format!("写入凭据失败: {e}"))
    }

    fn remove_one(name: &str) -> std::result::Result<bool, String> {
        match entry(name)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(format!("删除凭据失败: {e}")),
        }
    }

    pub fn set(name: &str, secret: &str) -> std::result::Result<(), String> {
        let chunks = split_secret(secret);
        if chunks.is_empty() {
            return delete(name);
        }
        if chunks.len() > MAX_CHUNKS {
            return Err("凭据太长,拆开之后仍然超过上限".into());
        }
        for (i, chunk) in chunks.iter().enumerate() {
            if let Err(e) = write_one(&part_name(name, i), chunk) {
                for j in 0..=i {
                    let _ = remove_one(&part_name(name, j));
                }
                return Err(e);
            }
        }
        // 这次比上次短:把多出来的旧分片删掉,不然读的时候会拼上过期的尾巴
        for i in chunks.len()..MAX_CHUNKS {
            if !remove_one(&part_name(name, i))? {
                break;
            }
        }
        Ok(())
    }

    pub fn get(name: &str) -> Option<String> {
        let mut out = String::new();
        for i in 0..MAX_CHUNKS {
            let Ok(ent) = entry(&part_name(name, i)) else {
                break;
            };
            match ent.get_password() {
                Ok(part) => out.push_str(&part),
                Err(_) => break,
            }
        }
        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    }

    pub fn delete(name: &str) -> std::result::Result<(), String> {
        for i in 0..MAX_CHUNKS {
            if !remove_one(&part_name(name, i))? {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod store {
    pub fn set(_name: &str, _secret: &str) -> std::result::Result<(), String> {
        Err("当前平台请用环境变量提供凭据".into())
    }
    pub fn get(_name: &str) -> Option<String> {
        None
    }
    pub fn delete(_name: &str) -> std::result::Result<(), String> {
        Err("当前平台请用环境变量提供凭据".into())
    }
}

/// 读凭据:keyring 命中就用它,否则回退到同名环境变量。
pub fn resolve(name: &str) -> Option<String> {
    if let Some(v) = store::get(name) {
        return Some(v);
    }
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

pub fn exists(name: &str) -> bool {
    resolve(name).is_some()
}

pub fn set(name: &str, secret: &str) -> std::result::Result<(), String> {
    if !is_allowed_name(name) {
        return Err(format!("未知的凭据条目:{name}"));
    }
    if secret.trim().is_empty() {
        return delete(name);
    }
    store::set(name, secret)
}

pub fn delete(name: &str) -> std::result::Result<(), String> {
    store::delete(name)
}

/// 全部已知条目的存在性 —— **只有布尔值,没有密钥**
pub fn list() -> Vec<(String, bool)> {
    KNOWN
        .iter()
        .map(|(name, _)| ((*name).to_string(), exists(name)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 环境变量回退:keyring 里没有时应当从环境变量读到。
    /// (测试不写 keyring,避免污染用户的凭据管理器。)
    #[test]
    fn env_var_is_the_fallback() {
        let name = "OTR_TEST_CREDENTIAL_PROBE";
        std::env::set_var(name, "  secret-from-env  ");
        assert_eq!(resolve(name).as_deref(), Some("  secret-from-env  "));
        std::env::remove_var(name);
        assert!(resolve(name).is_none());
    }

    /// 空白值等于没有:不能让一个空字符串冒充"已配置"
    #[test]
    fn blank_env_values_count_as_missing() {
        let name = "OTR_TEST_CREDENTIAL_BLANK";
        std::env::set_var(name, "   ");
        assert!(resolve(name).is_none());
        std::env::remove_var(name);
    }

    /// list() 只吐布尔值,形状固定
    #[test]
    fn list_exposes_presence_only() {
        let got = list();
        assert_eq!(got.len(), KNOWN.len());
        for (name, present) in got {
            assert!(!name.is_empty());
            let _: bool = present;
        }
    }

    /// 生成名可以写进解析链:keyring 没有时,同名环境变量能读回来。
    /// 不调用 keyring,避免把测试密钥写进用户的凭据管理器。
    #[test]
    fn generated_names_can_be_resolved_after_they_are_written() {
        let name = "limit.deepseek.deepseek-work.key";
        assert!(is_allowed_name(name));
        assert!(is_generated_name(name));
        std::env::set_var(name, "sk-from-env");
        assert_eq!(resolve(name).as_deref(), Some("sk-from-env"));
        std::env::remove_var(name);
        assert!(resolve(name).is_none());
    }

    /// 超长 Cookie 按 UTF-16 码元切开,每片都低于 Windows 凭据 blob 的 2560 字节,
    /// 拼回去必须和原文一致,而且不能把一个字符切成两半。
    #[test]
    fn long_secrets_split_under_the_windows_blob_limit() {
        let secret = "a".repeat(5000);
        let parts = split_secret(&secret);
        assert!(parts.len() > 1, "5000 字符必须拆开");
        for part in &parts {
            assert!(
                part.encode_utf16().count() * 2 <= 2560,
                "单片 UTF-16 字节数不能超过凭据 blob 上限"
            );
        }
        assert_eq!(parts.concat(), secret);

        let short = "sk-sp-short";
        assert_eq!(split_secret(short), vec![short.to_string()]);

        // 一个非 BMP 字符占 2 个 UTF-16 码元,不能切在它中间
        let wide = "😀".repeat(CHUNK_UTF16_UNITS);
        let parts = split_secret(&wide);
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), wide);
        for part in &parts {
            assert!(part.encode_utf16().count() * 2 <= 2560);
        }
    }

    /// 空名、路径穿越、以及不在生成规则里的自拟条目名,都不能写。
    #[test]
    fn random_credential_names_are_rejected() {
        assert!(!is_allowed_name(""));
        assert!(!is_allowed_name("../"));
        assert!(!is_allowed_name("limit.deepseek...key"));
        assert!(!is_allowed_name("DEEPSEEK_API_KEY_WORK"));
        assert!(is_allowed_name("DEEPSEEK_API_KEY"));
        assert!(set("../", "x").is_err(), "非法名字不能进凭据库");
        assert!(set("", "x").is_err());
    }

    /// 设空值等于删除,不会把空串写进凭据库
    #[test]
    fn setting_an_empty_secret_deletes_instead() {
        // 非 Windows 下 delete 返回 Err(平台提示);Windows 下是幂等成功。
        // 两种都不该把空串写进去。
        let _ = set("OTR_TEST_NEVER_WRITTEN", "");
        assert!(!exists("OTR_TEST_NEVER_WRITTEN"));
    }
}
