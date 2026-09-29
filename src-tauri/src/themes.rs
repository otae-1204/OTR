//! 用户主题目录的发现与读取。
//!
//! 只做文件系统层面的事:枚举 `<应用数据目录>/themes/`、限制大小、读出文本。
//! 清单的解析与校验在前端 `src/theme/validate.ts`(token 语义只有那一份权威),
//! 这里不理解 JSON 里写了什么。
//!
//! 安全边界:
//! - 前端不能传路径进来:命令没有参数,只枚举固定目录,不存在路径穿越;
//! - 目录项若是符号链接一律跳过,读取范围不会离开主题目录;
//! - 单文件超过 [`MAX_THEME_FILE_BYTES`] 只报错不读,主题总数上限 [`MAX_THEMES`];
//! - 只认两种布局:`themes/<name>.json` 与 `themes/<name>/theme.json`;隐藏项跳过。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 单个主题文件的大小上限。前端 `MAX_THEME_FILE_BYTES` 与此一致。
pub const MAX_THEME_FILE_BYTES: u64 = 256 * 1024;
/// 最多列出多少个主题(超出的按名字排序后截断)
pub const MAX_THEMES: usize = 64;
pub const THEME_DIR_NAME: &str = "themes";
pub const THEME_FILE_NAME: &str = "theme.json";

/// 托盘「恢复默认主题」之后发给前端的事件;与前端 `THEME_RESET_EVENT`(ThemeProvider.tsx)一致。
pub const RESET_EVENT: &str = "theme://reset";

/// 「恢复默认主题」时对每个窗口执行的固定脚本:清掉首帧缓存、移除受控的主题 `<style>` 与
/// `<html>` 上的主题变量,页面立刻回到 index.css 的兜底外观(= 默认主题)。
/// 与前端 `resetThemeDom()`(apply.ts)等价,但不依赖前端脚本还活着 —— 主题把界面弄得
/// 不可见、甚至前端的事件监听没挂上时也生效。编译期常量,不拼接任何输入;
/// 键名与前端 `THEME_CACHE_KEY` / `THEME_STYLE_ID` 一致(`npm run test:theme` 会对照)。
pub const RESET_SCRIPT: &str = r#"(function () {
  try { localStorage.removeItem("otr-theme-cache"); } catch (e) {}
  var s = document.getElementById("otr-theme-css");
  if (s) s.remove();
  var r = document.documentElement;
  r.removeAttribute("style");
  r.removeAttribute("data-theme");
})();"#;

/// 一个候选主题文件。`contents` 与 `error` 二选一。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFile {
    /// 文件的绝对路径(展示与诊断用)
    pub path: String,
    /// 文件名(去 `.json`)或目录名;清单校验失败时前端用它当展示名
    pub name: String,
    pub contents: Option<String>,
    pub error: Option<String>,
}

/// 主题目录:与 settings.json / radar.db 同级
pub fn themes_dir(app_data: &Path) -> PathBuf {
    app_data.join(THEME_DIR_NAME)
}

/// 建目录(已存在则无事发生),让用户能直接找到往哪放文件
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// 枚举候选:(展示名, 文件路径)。跳过符号链接、隐藏项、不认识的布局。
fn candidates(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_hidden(name) {
            continue;
        }
        // symlink_metadata 不跟随链接:链接本身就跳过,不管它指向哪
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_file() {
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
            {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name)
                    .to_string();
                out.push((stem, path));
            }
            continue;
        }
        if meta.is_dir() {
            let inner = path.join(THEME_FILE_NAME);
            let Ok(imeta) = std::fs::symlink_metadata(&inner) else {
                continue;
            };
            if imeta.file_type().is_symlink() || !imeta.is_file() {
                continue;
            }
            out.push((name.to_string(), inner));
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.1.cmp(&b.1)));
    out
}

fn read_one(name: String, path: PathBuf) -> ThemeFile {
    let path_str = path.to_string_lossy().to_string();
    let size = match std::fs::metadata(&path) {
        Ok(m) => m.len(),
        Err(e) => {
            return ThemeFile {
                path: path_str,
                name,
                contents: None,
                error: Some(format!("无法读取:{e}")),
            }
        }
    };
    if size > MAX_THEME_FILE_BYTES {
        return ThemeFile {
            path: path_str,
            name,
            contents: None,
            error: Some(format!(
                "文件 {} KiB,超过 {} KiB 上限,未读取",
                size / 1024,
                MAX_THEME_FILE_BYTES / 1024
            )),
        };
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => ThemeFile {
            path: path_str,
            name,
            contents: Some(text),
            error: None,
        },
        Err(e) => ThemeFile {
            path: path_str,
            name,
            contents: None,
            error: Some(if e.kind() == std::io::ErrorKind::InvalidData {
                "不是 UTF-8 文本".to_string()
            } else {
                format!("无法读取:{e}")
            }),
        },
    }
}

/// 枚举并读取主题目录里的所有候选文件。目录不存在时返回空表。
pub fn discover(dir: &Path) -> Vec<ThemeFile> {
    let mut list = candidates(dir);
    if list.len() > MAX_THEMES {
        eprintln!(
            "[otr] 主题目录里有 {} 个候选,只加载前 {} 个",
            list.len(),
            MAX_THEMES
        );
        list.truncate(MAX_THEMES);
    }
    list.into_iter().map(|(n, p)| read_one(n, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复位脚本只碰主题自己的三样东西(首帧缓存、受控 <style>、<html> 内联变量),
    /// 而且是个自执行的固定函数:没有格式化占位符、没有网络 / 存储的其它键
    #[test]
    fn reset_script_only_touches_theme_state() {
        assert!(RESET_SCRIPT.starts_with("(function () {") && RESET_SCRIPT.ends_with("})();"));
        assert!(RESET_SCRIPT.contains(r#"localStorage.removeItem("otr-theme-cache")"#));
        assert!(RESET_SCRIPT.contains(r#"getElementById("otr-theme-css")"#));
        assert!(RESET_SCRIPT.contains(r#"removeAttribute("style")"#));
        for banned in ["${", "fetch", "XMLHttpRequest", "clear()", "innerHTML", "eval"] {
            assert!(!RESET_SCRIPT.contains(banned), "复位脚本不该含 {banned}");
        }
        assert_eq!(
            RESET_SCRIPT.matches('{').count(),
            RESET_SCRIPT.matches('}').count()
        );
        assert_eq!(RESET_EVENT, "theme://reset");
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "otr-themes-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 两种布局都认,按名字排序;非 json、隐藏项、没有 theme.json 的目录都跳过
    #[test]
    fn discovers_both_layouts_in_name_order() {
        let dir = temp("layouts");
        std::fs::write(dir.join("zeta.json"), r#"{"id":"zeta"}"#).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::write(dir.join("alpha").join("theme.json"), r#"{"id":"alpha"}"#).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        std::fs::write(dir.join(".hidden.json"), r#"{}"#).unwrap();
        std::fs::create_dir_all(dir.join("empty-dir")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden-dir")).unwrap();
        std::fs::write(dir.join(".hidden-dir").join("theme.json"), r#"{}"#).unwrap();

        let found = discover(&dir);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
        assert_eq!(found[0].contents.as_deref(), Some(r#"{"id":"alpha"}"#));
        assert!(found[0].path.ends_with(THEME_FILE_NAME));
        assert_eq!(found[1].contents.as_deref(), Some(r#"{"id":"zeta"}"#));
        assert!(found.iter().all(|f| f.error.is_none()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超过大小上限的文件只报错,不把内容读进来
    #[test]
    fn oversized_files_are_reported_not_read() {
        let dir = temp("big");
        let big = vec![b' '; (MAX_THEME_FILE_BYTES + 1) as usize];
        std::fs::write(dir.join("big.json"), &big).unwrap();
        std::fs::write(dir.join("ok.json"), "{}").unwrap();

        let found = discover(&dir);
        assert_eq!(found.len(), 2);
        let big = found.iter().find(|f| f.name == "big").unwrap();
        assert!(big.contents.is_none());
        assert!(big.error.as_deref().unwrap_or("").contains("上限"), "{:?}", big.error);
        let ok = found.iter().find(|f| f.name == "ok").unwrap();
        assert_eq!(ok.contents.as_deref(), Some("{}"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 UTF-8 内容报错而不是 panic
    #[test]
    fn invalid_utf8_is_reported() {
        let dir = temp("utf8");
        std::fs::write(dir.join("bad.json"), [0xff, 0xfe, b'{', b'}']).unwrap();
        let found = discover(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].contents.is_none());
        assert!(found[0].error.as_deref().unwrap_or("").contains("UTF-8"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不存在:空表,不报错
    #[test]
    fn missing_dir_yields_empty_list() {
        let dir = temp("missing").join("nope");
        assert!(discover(&dir).is_empty());
    }

    /// 符号链接(文件或目录)一律跳过:读取范围不能离开主题目录
    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped() {
        let dir = temp("symlink");
        let outside = temp("outside");
        std::fs::write(outside.join("secret.json"), r#"{"id":"secret"}"#).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.json"), dir.join("link.json")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("linked-dir")).unwrap();
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.json"),
            dir.join("real").join(THEME_FILE_NAME),
        )
        .unwrap();
        std::fs::write(dir.join("fine.json"), "{}").unwrap();

        let found = discover(&dir);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["fine"]);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 超过上限只加载前 MAX_THEMES 个(按名字序)
    #[test]
    fn caps_the_number_of_themes() {
        let dir = temp("cap");
        for i in 0..(MAX_THEMES + 3) {
            std::fs::write(dir.join(format!("t{i:03}.json")), "{}").unwrap();
        }
        let found = discover(&dir);
        assert_eq!(found.len(), MAX_THEMES);
        assert_eq!(found[0].name, "t000");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_dir_creates_and_is_idempotent() {
        let base = temp("ensure");
        let dir = themes_dir(&base);
        assert!(dir.ends_with(THEME_DIR_NAME));
        ensure_dir(&dir).unwrap();
        ensure_dir(&dir).unwrap();
        assert!(dir.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }
}
