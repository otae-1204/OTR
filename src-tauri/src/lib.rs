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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use tauri::{AppHandle, Emitter, Manager};

/// rescan 的合并调度:同一时刻只有一个扫描线程,连点托盘只会合并成一轮。
///
/// 协议简单但容易写错,所以单独抽出来 + 配单测:
/// - 请求方 fetch_add 登记 pending,prev == 0 且抢占到 worker 的那个线程负责开线程;
/// - worker 每轮 swap(0) 取走全部 pending(合并),跑完再看有没有新请求;
/// - 收尾处必须"先注销 worker 再确认 pending",否则会丢掉刚好卡在缝隙里的请求。
#[derive(Default)]
pub struct RescanGate {
    pending: AtomicUsize,
    worker: AtomicBool,
    full: AtomicBool,
}

pub enum RescanStep {
    Run { full: bool },
    Stop,
}

impl RescanGate {
    /// 登记一次请求;返回 true 表示调用方应当启动 worker 线程
    pub fn request(&self, full: bool) -> bool {
        if full {
            self.full.store(true, Ordering::SeqCst);
        }
        let prev = self.pending.fetch_add(1, Ordering::SeqCst);
        prev == 0 && !self.worker.swap(true, Ordering::SeqCst)
    }

    /// worker 取一轮任务;Stop 表示没有待跑请求了,线程可以退出
    pub fn next(&self) -> RescanStep {
        loop {
            if self.pending.swap(0, Ordering::SeqCst) > 0 {
                return RescanStep::Run {
                    full: self.full.swap(false, Ordering::SeqCst),
                };
            }
            self.worker.store(false, Ordering::SeqCst);
            if self.pending.load(Ordering::SeqCst) == 0 {
                return RescanStep::Stop;
            }
            if self.worker.swap(true, Ordering::SeqCst) {
                return RescanStep::Stop;
            }
        }
    }
}

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
    /// rescan 去重:连点托盘不该排队一串全量扫描
    pub rescan: RescanGate,
}

#[derive(Default)]
pub struct ScanMeta {
    pub cursors: HashMap<String, HashMap<String, FileCursor>>,
    pub states: HashMap<String, serde_json::Value>,
}

/// 取锁并容忍中毒。
///
/// 后台扫描线程一旦 panic,Mutex 会被 poison,之后每次 `.lock().unwrap()` 都继续 panic:
/// watcher 线程就是这样死掉的 —— 而它一死,托盘的"立即刷新"和自动刷新会**永久静默失效**,
/// 用户完全看不出来。这里这些共享数据(游标 / Provider state / 设置 / SQLite 连接)都是
/// 可以继续使用的普通值,拿回内部值远比连锁 panic 安全。
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 从 panic payload 里取一句能打日志的话
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
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
            // 启动时最小化:设置里声明了很久,但以前 Rust 侧从来没有读过它
            let start_minimized = settings.start_minimized;
            app.manage(AppState {
                store,
                providers: providers::all_providers(),
                settings_path,
                settings: Mutex::new(settings),
                scan_meta: Mutex::new(ScanMeta::default()),
                scan_lock: Mutex::new(()),
                watcher: Mutex::new(None),
                rescan: RescanGate::default(),
            });
            tray::setup(app.handle())?;
            if start_minimized {
                // 托盘常驻应用:启动不弹主窗口,点托盘图标再打开
                if let Some(window) = app.get_webview_window("main") {
                    if let Err(e) = window.hide() {
                        eprintln!("[otr] 启动最小化失败: {e}");
                    }
                }
            }
            let handle = watcher::start(app.handle().clone())?;
            {
                let state = app.state::<AppState>();
                *lock(&state.watcher) = Some(handle.clone());
                let settings = lock(&state.settings).clone();
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

/// 串行执行扫描(需要重建时走 replace_agent 原子替换),成功后刷新托盘并通知前端。
/// panic 一律在这里被隔离:调用方是 watcher / 后台线程,线程死掉就再也没有自动刷新了。
pub fn run_scan(app: &AppHandle, full: bool, only: Option<&str>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_scan_inner(app, full, only)
    }));
    if let Err(payload) = result {
        eprintln!("[otr] 扫描 panic 已隔离: {}", panic_message(&*payload));
    }
}

fn run_scan_inner(app: &AppHandle, full: bool, only: Option<&str>) {
    let state = app.state::<AppState>();
    let _guard = lock(&state.scan_lock);
    let settings = lock(&state.settings).clone();
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
                    let meta = lock(&state.scan_meta);
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
                // 修历史:某些日期曾因"按天交给台账"而只写进了按小时表(轴表有数据、
                // 当天卡片恒为 0),而记录是增量语义、不会重放,只能在库里补回来。
                // 只填 usage_daily 整天没有行的日期,幂等,可安全每次重跑。
                if let Some(skip) = p.ledger_owned_dates(&st) {
                    match state.store.backfill_daily_from_hourly(p.id(), &skip) {
                        Ok(n) if n > 0 => {
                            eprintln!("[{}] 按小时表回填按天表 {} 行", p.id(), n);
                            changed = true;
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("[{}] 回填按天表: {}", p.id(), e),
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
    let mut meta = lock(scan_meta);
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
    // 单个 Provider 的 panic 不该带崩整轮扫描(别的 Agent 还等着扫)
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.scan(&mut ctx)))
        .unwrap_or_else(|payload| {
            Err(crate::error::AppError::Msg(format!(
                "panic: {}",
                panic_message(&*payload)
            )))
        });
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

    struct PanickingProvider;

    impl AgentProvider for PanickingProvider {
        fn id(&self) -> &str {
            "boom"
        }
        fn display_name(&self) -> &str {
            "Boom"
        }
        fn detect(&self) -> bool {
            true
        }
        fn watch_paths(&self) -> Vec<PathBuf> {
            vec![]
        }
        fn scan(&self, _ctx: &mut ScanCtx) -> crate::error::Result<Vec<crate::model::UsageRecord>> {
            panic!("provider exploded");
        }
    }

    /// Provider panic 必须被转成 Err,而不是顺着调用栈把 watcher 线程带崩
    /// (线程一死,托盘"立即刷新"和自动刷新会永久静默失效)。
    #[test]
    fn provider_panic_is_isolated_into_a_scan_error() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("otr-panic-{suffix}"));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("t.db")).unwrap();
        let meta = Mutex::new(ScanMeta::default());

        let (res, force_full) = scan_provider(&store, &meta, &PanickingProvider, false);
        let err = res.expect_err("panic 必须转成 Err");
        assert!(
            err.to_string().contains("provider exploded"),
            "错误信息要带上 panic 原因: {err}"
        );
        assert!(!force_full);

        // panic 在 catch_unwind 里被就地接住,guard 还在 scan_provider 栈上正常 drop,
        // 所以连中毒都不会发生 —— 扫描状态完全可用
        assert!(!meta.is_poisoned());
        let m = lock(&meta);
        assert!(
            m.cursors.get("boom").is_some_and(|c| c.is_empty()),
            "panic 的 Provider 不该在内存基线里留下半个游标"
        );
        drop(m);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 万一某次 panic 真的在持锁期间 unwind(guard 被 drop),Mutex 会中毒;
    /// crate::lock 必须能从中毒状态里拿回内部值,而不是继续 panic。
    #[test]
    fn poisoned_lock_is_recovered() {
        let m = Mutex::new(7u32);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = m.lock().unwrap();
            panic!("poison it");
        }));
        assert!(caught.is_err());
        assert!(m.is_poisoned(), "guard 在 unwind 中 drop 应当让锁中毒");
        assert_eq!(*lock(&m), 7, "crate::lock 必须能拿回内部值");
    }

    /// rescan 合并协议:连点只跑一轮,非全量请求遇到全量请求要升级成全量,且不丢请求。
    #[test]
    fn rescan_gate_coalesces_and_never_drops_a_request() {
        let gate = RescanGate::default();
        assert!(gate.request(false), "第一个请求应当成为 worker");
        assert!(!gate.request(false), "已有 worker 时只登记");
        assert!(!gate.request(false));
        assert!(!gate.request(true), "全量请求也只登记");
        match gate.next() {
            RescanStep::Run { full } => assert!(full, "非全量请求遇到全量请求应当升级,而不是各跑一遍"),
            RescanStep::Stop => panic!("还有待跑请求,不该 Stop"),
        }
        assert!(matches!(gate.next(), RescanStep::Stop), "pending 已清空");

        // 注销之后新请求可以重新抢占
        assert!(gate.request(false));
        assert!(matches!(gate.next(), RescanStep::Run { full: false }));
        assert!(matches!(gate.next(), RescanStep::Stop));
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
