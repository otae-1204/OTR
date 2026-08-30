use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tauri::AppHandle;

use crate::error::Result;
use crate::{run_scan, AppState};

/// 句柄持有者可在运行时更换监听目标(设置里启停/增删自定义 Agent 后调用)
#[derive(Clone)]
pub struct WatcherHandle {
    tx: mpsc::Sender<Vec<PathBuf>>,
}

impl WatcherHandle {
    pub fn rewatch(&self, paths: Vec<PathBuf>) {
        let _ = self.tx.send(paths);
    }
}

/// 汇总当前应监听的目录:内置 + 自定义,只含已启用且检测到的
pub fn current_watch_paths(state: &AppState, settings: &crate::settings::Settings) -> Vec<PathBuf> {
    let customs = crate::providers::build_customs(settings);
    let mut out = Vec::new();
    for p in state
        .providers
        .iter()
        .map(|b| b.as_ref())
        .chain(customs.iter().map(|b| b.as_ref()))
    {
        if !settings.is_enabled(p.id()) || !p.detect() {
            continue;
        }
        out.extend(p.watch_paths());
    }
    out
}

/// 监听所有已启用 Provider 的数据目录;防抖 2 秒后触发一次增量扫描。
/// 收到 rewatch 消息时整体重建 watcher(旧监听随实例 drop 注销)
pub fn start(app: AppHandle) -> Result<WatcherHandle> {
    let (path_tx, path_rx) = mpsc::channel::<Vec<PathBuf>>();
    let (evt_tx, evt_rx) = mpsc::channel::<notify::Result<notify::Event>>();

    std::thread::spawn(move || {
        let make_watcher = || {
            notify::recommended_watcher({
                let tx = evt_tx.clone();
                move |res| {
                    let _ = tx.send(res);
                }
            })
            .ok()
        };
        // 阻塞等待首组监听目标(setup 后立刻会收到),之后在循环里响应 rewatch
        let mut paths: Vec<PathBuf> = match path_rx.recv() {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut watcher = make_watcher();
        if let Some(w) = watcher.as_mut() {
            for p in &paths {
                if p.exists() {
                    if let Err(e) = w.watch(p, RecursiveMode::Recursive) {
                        eprintln!("[watch] {}: {}", p.display(), e);
                    }
                }
            }
        }
        let mut pending = false;
        let mut last_scan = Instant::now() - Duration::from_secs(10);

        loop {
            // 运行时更换监听目标
            if let Ok(new_paths) = path_rx.try_recv() {
                paths = new_paths;
                watcher = make_watcher();
                if let Some(w) = watcher.as_mut() {
                    for p in &paths {
                        if p.exists() {
                            if let Err(e) = w.watch(p, RecursiveMode::Recursive) {
                                eprintln!("[watch] {}: {}", p.display(), e);
                            }
                        }
                    }
                }
            }

            // 事件防抖;空闲时 1.5s 醒一次检查 rewatch 队列
            let got = if pending {
                evt_rx.recv_timeout(Duration::from_millis(500)).is_ok()
            } else {
                match evt_rx.recv_timeout(Duration::from_millis(1500)) {
                    Ok(_) => true,
                    Err(mpsc::RecvTimeoutError::Timeout) => false,
                    Err(_) => break,
                }
            };
            if got {
                while evt_rx.try_recv().is_ok() {}
                pending = true;
            }

            if pending && last_scan.elapsed() >= Duration::from_secs(2) {
                pending = false;
                last_scan = Instant::now();
                run_scan(&app, false, None);
            }
        }
    });
    Ok(WatcherHandle { tx: path_tx })
}
