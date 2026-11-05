//! Оболочка приложения: tray, глобальная клавиша, оверлей, автозапуск,
//! восстановление таймеров после перезапуска.

use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Показать/спрятать оверлей (compact always-on-top). Создаётся по требованию.
pub fn toggle_overlay(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
        return;
    }
    let overlay = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html#overlay".into()))
        .title("JARVIS")
        .inner_size(420.0, 150.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .transparent(false)
        .center()
        .build();
    if let Err(e) = overlay {
        eprintln!("[overlay] {}", e);
    }
}

/// Перерегистрация глобальной клавиши оверлея.
pub fn rebind_hotkey(app: &AppHandle, accel: &str) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if accel.is_empty() {
        return;
    }
    let parsed: Result<tauri_plugin_global_shortcut::Shortcut, _> = accel.parse();
    match parsed {
        Ok(shortcut) => {
            let app2 = app.clone();
            if let Err(e) = gs.on_shortcut(shortcut, move |_app, _sc, event| {
                if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                    toggle_overlay(&app2);
                }
            }) {
                eprintln!("[hotkey] {}: {}", accel, e);
            }
        }
        Err(e) => eprintln!("[hotkey] не разобрал «{}»: {}", accel, e),
    }
}

/// Tray-меню: Показать / Оверлей / Выход.
pub fn build_tray(app: &AppHandle) {
    let show = MenuItem::with_id(app, "show", "Показать JARVIS", true, None::<&str>).unwrap();
    let overlay = MenuItem::with_id(app, "overlay", "Оверлей (клавишей)", true, None::<&str>).unwrap();
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>).unwrap();
    let menu = Menu::with_items(app, &[&show, &overlay, &quit]).unwrap();
    if let Some(tray) = app.tray_by_id("jarvis") {
        let _ = tray.set_menu(Some(menu));
        let app2 = app.clone();
        tray.on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "overlay" => toggle_overlay(&app2),
            "quit" => {
                app.state::<crate::AppState>().quit.store(true, std::sync::atomic::Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        });
        // Клик по иконке — показать окно.
        tray.on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
        });
    }
}

/// Восстановить незавершённые таймеры после перезапуска.
pub fn restore_timers(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let Ok(pending) = state.db.timers_pending() else { return };
    let now = chrono::Utc::now();
    for (id, name, ends) in pending {
        let Ok(ends_at) = chrono::DateTime::parse_from_rfc3339(&ends) else { continue };
        let ends_at = ends_at.with_timezone(&chrono::Utc);
        if ends_at > now {
            crate::tools::spawn_timer(app.clone(), id, name, ends_at);
        } else {
            // Просроченный: помечаем сработавшим, чтобы не копились.
            let _ = state.db.timer_mark_fired(id);
        }
    }
}
