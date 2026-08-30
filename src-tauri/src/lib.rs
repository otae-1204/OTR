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

use providers::{FileCursor, ScanCtx, AgentProvider};
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
                                    let _ = std::fs::copy(&from, dir.join(format!("radar.db{ext}")));
                                }
                            }
                            let _ =
                                std::fs::copy(legacy.join("settings.json"), dir.join("settings.json"));
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
                run_scan(&handle, true, None);
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

/// 串行执行增量扫描(全量时先 wipe),成功后刷新托盘并通知前端
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
        let result = {
            let mut meta = state.scan_meta.lock().unwrap();
            if full {
                let _ = state.store.wipe_agent(p.id());
                meta.cursors.remove(p.id());
                meta.states.remove(p.id());
            }
            // 通过 &mut 引用做字段级拆分借用(直接在 MutexGuard 上连续借用两个字段会 E0499)
            let m: &mut ScanMeta = &mut meta;
            let cursors = m.cursors.entry(p.id().to_string()).or_default();
            let mut st = m
                .states
                .remove(p.id())
                .unwrap_or(serde_json::Value::Null);
            let mut ctx = ScanCtx {
                full,
                cursors,
                state: &mut st,
            };
            let res = p.scan(&mut ctx);
            m.states.insert(p.id().to_string(), st);
            res
        };
        match result {
            Ok(records) => {
                if !records.is_empty() {
                    match state.store.apply_records(&records) {
                        Ok(n) => changed = changed || n > 0,
                        Err(e) => eprintln!("[{}] apply: {}", p.id(), e),
                    }
                }
                let meta = state.scan_meta.lock().unwrap();
                if let Some(cmap) = meta.cursors.get(p.id()) {
                    for (path, cur) in cmap {
                        let _ = state.store.set_cursor(p.id(), path, cur);
                    }
                }
                if let Some(st) = meta.states.get(p.id()) {
                    if let Ok(s) = serde_json::to_string(st) {
                        let _ = state
                            .store
                            .set_kv(&format!("state:{}", p.id()), &s);
                    }
                }
            }
            Err(e) => eprintln!("[{}] scan: {}", p.id(), e),
        }
    }

    if changed || full {
        tray::update_today_tooltip(app);
        let _ = app.emit("usage://updated", ());
    }
}
