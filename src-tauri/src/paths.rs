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
    home().join(".dsh").join("storages")
}

pub fn opencode_db() -> PathBuf {
    home()
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db")
}
