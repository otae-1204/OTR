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

pub fn opencode_db() -> PathBuf {
    home()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}
