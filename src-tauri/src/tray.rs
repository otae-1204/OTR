use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{run_scan, themes, AppState};

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let rescan = MenuItem::with_id(app, "rescan", "立即刷新", true, None::<&str>)?;
    let reset_theme =
        MenuItem::with_id(app, "reset-theme", "恢复默认主题", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &rescan, &reset_theme, &quit])?;
    TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("no app icon").clone())
        .tooltip("OTR")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "rescan" => {
                let handle = app.clone();
                std::thread::spawn(move || run_scan(&handle, false, None));
            }
            "reset-theme" => reset_theme_to_default(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 托盘「恢复默认主题」:主题把界面弄坏(看不见字、设置页点不到)时的兜底入口。
///
/// Rust 直接改设置(不经过前端):`themeId` 回到内置 `otr`,生效模式回到偏好模式,其余设置不动。
/// 然后对每个窗口执行固定的复位脚本(前端脚本坏了也生效),再发 `theme://reset` 让前端按新设置
/// 重新应用,最后把主窗口叫出来让用户看到结果。
fn reset_theme_to_default(app: &AppHandle) {
    let state = app.state::<AppState>();
    {
        let mut settings = crate::lock(&state.settings);
        settings.reset_theme();
        if let Err(e) = settings.save(&state.settings_path) {
            // 写盘失败也保留内存里的新值:本次运行立即生效(前端读的就是它),下次保存设置时再落盘
            eprintln!("[otr] 恢复默认主题:保存设置失败: {e}");
        }
    }
    for win in app.webview_windows().values() {
        if let Err(e) = win.eval(themes::RESET_SCRIPT) {
            eprintln!("[otr] 恢复默认主题:窗口 {} 执行复位脚本失败: {e}", win.label());
        }
    }
    let _ = app.emit(themes::RESET_EVENT, ());
    show_main(app);
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

pub fn update_today_tooltip(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(t) = state.store.totals_for_date(&crate::model::today_str()) else {
        return;
    };
    let tooltip = format!(
        "OTR · 今日 {} tokens",
        crate::model::fmt_tokens(t.total_tokens)
    );
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(tooltip.as_str()));
    }
}
