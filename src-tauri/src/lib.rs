pub mod commands;
pub mod error;
pub mod model;
pub mod paths;
pub mod providers;
pub mod settings;
pub mod store;
pub mod tray;
pub mod watcher;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use providers::{AgentProvider, FileCursor, ScanCtx};
use settings::Settings;
use store::Store;

pub struct AppState {
    pub store: Store,
    pub providers: Vec<Box<dyn AgentProvider>>,
    pub settings_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub scan_meta: Mutex<ScanMeta>,
    pub scan_lock: Mutex<()>,
    pub watcher: Mutex<Option<watcher::WatcherHandle>>,
}

#[derive(Default)]
pub struct ScanMeta {
    pub cursors: HashMap<String, HashMap<String, FileCursor>>,
    pub states: HashMap<String, serde_json::Value>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // 历次改名迁移:com.otae.app / com.token-show.app 的数据搬进当前目录(旧应用需已关闭)
            if !dir.join("radar.db").exists() {
                'migration: for legacy_name in ["com.otae.app", "com.token-show.app"] {
                    let Some(legacy) = dir.parent().map(|p| p.join(legacy_name)) else {
                        continue;
                    };
                    if !legacy.is_dir() {
                        continue;
                    }
                    for db_name in ["otae.db", "token-show.db"] {
                        let src = legacy.join(db_name);
                        if src.exists() {
                            for ext in ["", "-wal", "-shm"] {
                                let from = legacy.join(format!("{db_name}{ext}"));
                                if from.exists() {
                                    let _ =
                                        std::fs::copy(&from, dir.join(format!("radar.db{ext}")));
                                }
                            }
                            let _ = std::fs::copy(
                                legacy.join("settings.json"),
                                dir.join("settings.json"),
                            );
                            break 'migration;
                        }
                    }
                }
            }
            let store = Store::open(&dir.join("radar.db"))?;
            let settings_path = dir.join("settings.json");
            let settings = Settings::load(&settings_path);
            app.manage(AppState {
                store,
                providers: providers::all_providers(),
                settings_path,
                settings: Mutex::new(settings),
                scan_meta: Mutex::new(ScanMeta::default()),
                scan_lock: Mutex::new(()),
                watcher: Mutex::new(None),
            });
            tray::setup(app.handle())?;
            let handle = watcher::start(app.handle().clone())?;
            {
                let state = app.state::<AppState>();
                *state.watcher.lock().unwrap() = Some(handle.clone());
                let settings = state.settings.lock().unwrap().clone();
                handle.rewatch(watcher::current_watch_paths(&state, &settings));
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(400));
                run_scan(&handle, false, None);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_agents,
            commands::get_summary,
            commands::get_range_summary,
            commands::get_daily,
            commands::get_sessions,
            commands::rescan,
            commands::get_settings,
            commands::list_models,
            commands::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running OTR");
}

/// 串行执行扫描(需要重建时走 replace_agent 原子替换),成功后刷新托盘并通知前端
pub fn run_scan(app: &AppHandle, full: bool, only: Option<&str>) {
    let state = app.state::<AppState>();
    let _guard = state.scan_lock.lock().unwrap();
    let settings = state.settings.lock().unwrap().clone();
    let customs = providers::build_customs(&settings);
    let all: Vec<&dyn AgentProvider> = state
        .providers
        .iter()
        .map(|b| b.as_ref())
        .chain(customs.iter().map(|b| b.as_ref()))
        .collect();
    let mut changed = false;
    let mut did_full_scan = false;
    let mut snapshot_done = false;

    for p in all {
        if let Some(want) = only {
            if p.id() != want {
                continue;
            }
        }
        if !settings.enabled_agents.iter().any(|a| a == p.id()) {
            continue;
        }
        if !p.detect() {
            continue;
        }
        let pv = p.parser_version();
        let pv_key = format!("parser_version:{}", p.id());
        let provider_full =
            full || state.store.get_kv(&pv_key).as_deref() != Some(pv.to_string().as_str());
        did_full_scan |= provider_full;

        // 版本升级导致的重建:先落一份一致性快照,失败不阻断
        if provider_full && !full && !snapshot_done {
            snapshot_done = true;
            if let Some(dir) = state.settings_path.parent() {
                let dest = dir.join(format!(
                    "radar.db.bak-{}",
                    chrono::Local::now().format("%Y%m%d%H%M%S")
                ));
                match state.store.snapshot(&dest) {
                    Ok(()) => eprintln!("[otr] 重建前快照: {}", dest.display()),
                    Err(e) => eprintln!("[otr] 快照失败(继续重建): {}", e),
                }
            }
        }
        // 扫描一次;解析器若发现数据源被截断/重写,会要求升级为全量重建后重跑
        let (mut result, force_full) =
            scan_provider(&state.store, &state.scan_meta, p, provider_full);
        let mut effective_full = provider_full;
        if force_full && !effective_full {
            eprintln!("[{}] 数据源被截断/重写,转为全量重建", p.id());
            effective_full = true;
            (result, _) = scan_provider(&state.store, &state.scan_meta, p, true);
        }
        did_full_scan |= effective_full;

        match result {
            Ok(records) => {
                let (cursors, st) = {
                    let meta = state.scan_meta.lock().unwrap();
                    (
                        meta.cursors.get(p.id()).cloned().unwrap_or_default(),
                        meta.states
                            .get(p.id())
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                };
                let applied = if effective_full {
                    // 原子替换:清空该 Agent 旧行 + 写入新结果 + 新基线,同一事务
                    state.store.replace_agent(p.id(), &records, &cursors, &st)
                } else {
                    state.store.apply_records(&records)
                };
                match applied {
                    Ok(n) => changed = changed || n > 0,
                    Err(e) => eprintln!("[{}] apply: {}", p.id(), e),
                }
                // replace_agent 已在同一事务内落盘游标与状态;增量路径这里补写
                if !effective_full {
                    for (path, cur) in &cursors {
                        let _ = state.store.set_cursor(p.id(), path, cur);
                    }
                    if let Ok(s) = serde_json::to_string(&st) {
                        let _ = state.store.set_kv(&format!("state:{}", p.id()), &s);
                    }
                }
                let _ = state.store.set_kv(&pv_key, &pv.to_string());
            }
            // 扫描失败:用量表一行没动,内存基线已在 scan_provider 内回滚
            Err(e) => eprintln!("[{}] scan: {}", p.id(), e),
        }
    }

    if changed || full || did_full_scan {
        tray::update_today_tooltip(app);
        let _ = app.emit("usage://updated", ());
    }
}

/// 跑一次扫描:加载/重置基线 → 执行 → 失败回滚内存基线。
/// 返回 (扫描结果, Provider 是否要求升级为全量重建)。
/// 只依赖 Store 与 ScanMeta(不碰用量数据表),便于单测。
fn scan_provider(
    store: &Store,
    scan_meta: &Mutex<ScanMeta>,
    p: &dyn AgentProvider,
    full: bool,
) -> (crate::error::Result<Vec<crate::model::UsageRecord>>, bool) {
    let id = p.id().to_string();
    let mut meta = scan_meta.lock().unwrap();
    meta.cursors
        .entry(id.clone())
        .or_insert_with(|| store.load_cursors(&id));
    meta.states.entry(id.clone()).or_insert_with(|| {
        store
            .get_kv(&format!("state:{id}"))
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or(serde_json::Value::Null)
    });

    // full 时把基线清零;失败必须回滚,否则下次扫描拿空基线重放 → 双计
    let saved = if full {
        Some((
            meta.cursors.get(&id).cloned().unwrap_or_default(),
            meta.states
                .get(&id)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        ))
    } else {
        None
    };
    if full {
        meta.cursors.insert(id.clone(), HashMap::new());
        meta.states.insert(id.clone(), serde_json::Value::Null);
    }

    // 通过 &mut 引用做字段级拆分借用(直接在 MutexGuard 上连续借用两个字段会 E0499)
    let m: &mut ScanMeta = &mut meta;
    let cursors = m.cursors.get_mut(&id).expect("just inserted");
    let mut st = m.states.remove(&id).unwrap_or(serde_json::Value::Null);
    let mut ctx = ScanCtx {
        full,
        force_full: false,
        cursors,
        state: &mut st,
    };
    let res = p.scan(&mut ctx);
    let force_full = ctx.force_full;
    m.states.insert(id.clone(), st);

    if res.is_err() {
        if let Some((cursors, state)) = saved {
            m.cursors.insert(id.clone(), cursors);
            m.states.insert(id.clone(), state);
        }
    }
    (res, force_full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AgentProvider, ScanCtx};
    use std::path::PathBuf;

    struct FailingProvider;

    impl AgentProvider for FailingProvider {
        fn id(&self) -> &str {
            "fake"
        }
        fn display_name(&self) -> &str {
            "Fake"
        }
        fn detect(&self) -> bool {
            true
        }
        fn watch_paths(&self) -> Vec<PathBuf> {
            vec![]
        }
        fn scan(&self, _ctx: &mut ScanCtx) -> crate::error::Result<Vec<crate::model::UsageRecord>> {
            Err(crate::error::AppError::Msg("boom".into()))
        }
    }

    #[test]
    fn failed_full_scan_restores_baseline() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("otr-lib-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("t.db")).unwrap();
        store.set_kv("state:fake", "{\"v\":1}").unwrap();
        store
            .set_cursor(
                "fake",
                "p",
                &FileCursor {
                    offset: 7,
                    ..Default::default()
                },
            )
            .unwrap();

        let meta = Mutex::new(ScanMeta::default());
        let (res, force_full) = scan_provider(&store, &meta, &FailingProvider, true);
        assert!(res.is_err());
        assert!(!force_full);

        let m = meta.lock().unwrap();
        assert_eq!(
            m.cursors.get("fake").unwrap().get("p").unwrap().offset,
            7,
            "失败必须回滚内存游标,否则下次扫描从 0 重放 → 双计"
        );
        assert_eq!(m.states.get("fake").unwrap(), &serde_json::json!({"v": 1}));
        drop(m);
        let _ = std::fs::remove_dir_all(dir);
    }
}
