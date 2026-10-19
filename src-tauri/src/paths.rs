use std::path::PathBuf;

/// 各 Agent 数据目录;本机 Windows 实测路径
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn claude_projects() -> PathBuf {
    home().join(".claude").join("projects")
}

pub fn codex_sessions() -> PathBuf {
    home().join(".codex").join("sessions")
}

pub fn codex_archived_sessions() -> PathBuf {
    home().join(".codex").join("archived_sessions")
}

pub fn zcode_rollout() -> PathBuf {
    home().join(".zcode").join("cli").join("rollout")
}

pub fn zcode_agents() -> PathBuf {
    home().join(".zcode").join("cli").join("agents")
}

pub fn pi_sessions() -> PathBuf {
    home().join(".pi").join("agent").join("sessions")
}

/// Grok Build 的家目录。官方变量是 `GROK_HOME`,缺省 `~/.grok`。
pub fn grok_home() -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".grok"))
}

pub fn grok_sessions() -> PathBuf {
    grok_home().join("sessions")
}

pub fn dsh_storages() -> PathBuf {
    dsh_home().join("storages")
}

/// DSH 与宿主一致的单根目录:优先使用 DSH_HOME,否则回退到 ~/.dsh。
pub fn dsh_home() -> PathBuf {
    std::env::var_os("DSH_HOME")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".dsh"))
}

/// OpenCode 的 SQLite。路径随平台和版本变过,用第一个已经存在的文件。
///
/// - `~/.local/share/opencode/opencode.db`:Linux,以及一部分 Windows / 旧安装
/// - `XDG_DATA_HOME/opencode/opencode.db`:显式设置了 XDG 时
/// - `dirs::data_dir()/opencode/opencode.db`:macOS 的 Application Support、Windows 的 Roaming
pub fn opencode_db() -> PathBuf {
    let candidates = opencode_db_candidates();
    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .unwrap_or_else(|| {
            candidates.into_iter().next().unwrap_or_else(|| {
                home()
                    .join(".local")
                    .join("share")
                    .join("opencode")
                    .join("opencode.db")
            })
        })
}

fn opencode_db_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |path: PathBuf| {
        if !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    };
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let xdg = PathBuf::from(xdg);
        if !xdg.as_os_str().is_empty() {
            push(xdg.join("opencode").join("opencode.db"));
        }
    }
    push(
        home()
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db"),
    );
    if let Some(data) = dirs::data_dir() {
        push(data.join("opencode").join("opencode.db"));
    }
    if let Some(local) = dirs::data_local_dir() {
        push(local.join("opencode").join("opencode.db"));
    }
    out
}

/// Cursor IDE 用户目录:Windows %APPDATA%\Cursor,macOS Application Support,Linux ~/.config/Cursor
pub fn cursor_user_dir() -> PathBuf {
    #[cfg(windows)]
    {
        return dirs::data_dir()
            .unwrap_or_else(|| home().join("AppData").join("Roaming"))
            .join("Cursor");
    }
    #[cfg(target_os = "macos")]
    {
        return home()
            .join("Library")
            .join("Application Support")
            .join("Cursor");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("Cursor")
    }
}

pub fn cursor_state_db() -> PathBuf {
    cursor_user_dir()
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}
