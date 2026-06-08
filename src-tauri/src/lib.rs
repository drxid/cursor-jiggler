use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use enigo::{Coordinate, Enigo, Mouse, Settings};
use serde::Serialize;
use serde_json::json;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent, Wry,
};
use tauri_plugin_store::StoreExt;

const STORE_FILE: &str = "settings.json";
const DEFAULT_INTERVAL: u64 = 60;
const TRAY_NORMAL: &[u8] = include_bytes!("../icons/tray-normal.png");
const TRAY_ACTIVE: &[u8] = include_bytes!("../icons/tray-active.png");

/// Localizable strings for the tray, pushed from the frontend i18n layer.
#[derive(Clone, serde::Deserialize)]
struct Labels {
    status_active: String,
    status_inactive: String,
    enable: String,
    disable: String,
    show: String,
    quit: String,
    tooltip_active: String,
    tooltip_inactive: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            status_active: "● Active".into(),
            status_inactive: "○ Inactive".into(),
            enable: "Enable".into(),
            disable: "Disable".into(),
            show: "Show window".into(),
            quit: "Quit".into(),
            tooltip_active: "Cursor Jiggler — active".into(),
            tooltip_inactive: "Cursor Jiggler — inactive".into(),
        }
    }
}

/// Handles to the tray menu items we update as state / language changes.
struct TrayUi {
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    show: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// Reflect the on/off state everywhere: the tray icon and the window/taskbar
/// icon both gain a green dot when active; tooltip and status / toggle labels
/// follow the current state and the selected language.
fn update_status_ui(app: &AppHandle, enabled: bool) {
    let labels = app
        .try_state::<Mutex<Labels>>()
        .and_then(|s| s.lock().ok().map(|l| l.clone()))
        .unwrap_or_default();

    let bytes = if enabled { TRAY_ACTIVE } else { TRAY_NORMAL };

    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Ok(img) = Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_tooltip(Some(if enabled {
            labels.tooltip_active.clone()
        } else {
            labels.tooltip_inactive.clone()
        }));
    }

    // Window icon — shows in the taskbar and alt-tab.
    if let Some(win) = app.get_webview_window("main") {
        if let Ok(img) = Image::from_bytes(bytes) {
            let _ = win.set_icon(img);
        }
    }

    if let Some(state) = app.try_state::<Mutex<TrayUi>>() {
        if let Ok(ui) = state.lock() {
            let _ = ui.status.set_text(if enabled {
                &labels.status_active
            } else {
                &labels.status_inactive
            });
            let _ = ui.toggle.set_text(if enabled {
                &labels.disable
            } else {
                &labels.enable
            });
        }
    }
}

/// Shared, lock-free state for the jiggle engine.
#[derive(Default)]
struct Core {
    enabled: AtomicBool,
    zen: AtomicBool,
    interval: AtomicU64,
    remaining: AtomicU64,
    start_minimized: AtomicBool,
}

type SharedCore = Arc<Core>;

#[derive(Serialize, Clone)]
struct Status {
    enabled: bool,
    zen: bool,
    interval: u64,
    remaining: u64,
    #[serde(rename = "startMinimized")]
    start_minimized: bool,
}

fn snapshot(c: &Core) -> Status {
    Status {
        enabled: c.enabled.load(Ordering::Relaxed),
        zen: c.zen.load(Ordering::Relaxed),
        interval: c.interval.load(Ordering::Relaxed),
        remaining: c.remaining.load(Ordering::Relaxed),
        start_minimized: c.start_minimized.load(Ordering::Relaxed),
    }
}

/// Nudge the cursor and return it to its original position so we never drift.
/// Zen mode moves by a single pixel; normal mode by a clearly visible amount.
fn jiggle(zen: bool) {
    let Ok(mut enigo) = Enigo::new(&Settings::default()) else {
        return;
    };
    let delta = if zen { 1 } else { 15 };
    if let Ok((x, y)) = enigo.location() {
        let _ = enigo.move_mouse(x + delta, y + delta, Coordinate::Abs);
        std::thread::sleep(Duration::from_millis(40));
        let _ = enigo.move_mouse(x, y, Coordinate::Abs);
    }
}

/// How many seconds the user has been inactive. On Windows this is the real
/// system-wide idle time (mouse *and* keyboard) via `GetLastInputInfo`; on other
/// platforms we approximate it by watching the cursor position between ticks.
#[derive(Default)]
struct IdleTracker {
    idle: u64,
    last_pos: Option<(i32, i32)>,
}

impl IdleTracker {
    #[cfg(windows)]
    fn seconds(&mut self) -> u64 {
        win_idle_seconds()
    }

    #[cfg(not(windows))]
    fn seconds(&mut self) -> u64 {
        if let Ok(mut enigo) = Enigo::new(&Settings::default()) {
            if let Ok(pos) = enigo.location() {
                match self.last_pos {
                    Some(prev) if prev != pos => self.idle = 0, // user moved the cursor
                    _ => self.idle = self.idle.saturating_add(1),
                }
                self.last_pos = Some(pos);
            }
        }
        self.idle
    }

    fn reset(&mut self) {
        self.idle = 0;
        self.last_pos = None;
    }
}

/// System-wide idle time in seconds (Windows). Any mouse move or key press
/// resets it to ~0, so we never jiggle while the computer is in use.
#[cfg(windows)]
fn win_idle_seconds() -> u64 {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }
    extern "system" {
        fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
        fn GetTickCount() -> u32;
    }
    unsafe {
        let mut lii = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        if GetLastInputInfo(&mut lii) != 0 {
            let idle_ms = GetTickCount().wrapping_sub(lii.dw_time);
            (idle_ms / 1000) as u64
        } else {
            0
        }
    }
}

/// Keep the system (and display) awake while the jiggler is active. Driven from
/// the long-lived loop thread, since the ES_CONTINUOUS flag is per-thread.
#[cfg(windows)]
fn set_keep_awake(active: bool) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
    extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }
    let flags = if active {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe {
        SetThreadExecutionState(flags);
    }
}

#[cfg(not(windows))]
fn set_keep_awake(_active: bool) {}

/// Background heartbeat: ticks once per second. It only jiggles once the user
/// has been idle for the chosen interval, and resets the countdown the moment
/// any activity is detected — so it never gets in the way while you're working.
fn spawn_loop(app: AppHandle, core: SharedCore) {
    std::thread::spawn(move || {
        let mut tracker = IdleTracker::default();
        let mut awake = false;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let enabled = core.enabled.load(Ordering::Relaxed);

            // Toggle the keep-awake lock when the active state changes.
            if enabled != awake {
                set_keep_awake(enabled);
                awake = enabled;
            }

            if enabled {
                let interval = core.interval.load(Ordering::Relaxed);
                let idle = tracker.seconds();
                if idle >= interval {
                    jiggle(core.zen.load(Ordering::Relaxed));
                    tracker.reset();
                    core.remaining.store(interval, Ordering::Relaxed);
                } else {
                    core.remaining
                        .store(interval.saturating_sub(idle), Ordering::Relaxed);
                }
            }
            let _ = app.emit("status", snapshot(&core));
        }
    });
}

/// Persist the current settings (not the volatile countdown) to disk.
fn persist(app: &AppHandle, core: &Core) {
    if let Ok(store) = app.store(STORE_FILE) {
        store.set("enabled", json!(core.enabled.load(Ordering::Relaxed)));
        store.set("zen", json!(core.zen.load(Ordering::Relaxed)));
        store.set("interval", json!(core.interval.load(Ordering::Relaxed)));
        store.set(
            "start_minimized",
            json!(core.start_minimized.load(Ordering::Relaxed)),
        );
        let _ = store.save();
    }
}

#[tauri::command]
fn get_status(core: State<SharedCore>) -> Status {
    snapshot(&core)
}

#[tauri::command]
fn set_enabled(enabled: bool, app: AppHandle, core: State<SharedCore>) -> Status {
    core.enabled.store(enabled, Ordering::Relaxed);
    if enabled {
        core.remaining
            .store(core.interval.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    persist(&app, &core);
    update_status_ui(&app, enabled);
    snapshot(&core)
}

#[tauri::command]
fn set_zen(zen: bool, app: AppHandle, core: State<SharedCore>) -> Status {
    core.zen.store(zen, Ordering::Relaxed);
    persist(&app, &core);
    snapshot(&core)
}

#[tauri::command]
fn set_interval(seconds: u64, app: AppHandle, core: State<SharedCore>) -> Status {
    let s = seconds.max(1);
    core.interval.store(s, Ordering::Relaxed);
    core.remaining.store(s, Ordering::Relaxed);
    persist(&app, &core);
    snapshot(&core)
}

#[tauri::command]
fn set_start_minimized(value: bool, app: AppHandle, core: State<SharedCore>) -> Status {
    core.start_minimized.store(value, Ordering::Relaxed);
    persist(&app, &core);
    snapshot(&core)
}

/// Push localized tray strings from the frontend and re-render the tray.
#[tauri::command]
fn set_labels(labels: Labels, app: AppHandle, core: State<SharedCore>) {
    if let Some(state) = app.try_state::<Mutex<Labels>>() {
        if let Ok(mut current) = state.lock() {
            *current = labels.clone();
        }
    }
    if let Some(ui) = app.try_state::<Mutex<TrayUi>>() {
        if let Ok(ui) = ui.lock() {
            let _ = ui.show.set_text(&labels.show);
            let _ = ui.quit.set_text(&labels.quit);
        }
    }
    update_status_ui(&app, core.enabled.load(Ordering::Relaxed));
}

fn show_window(app: &AppHandle) {
    // No re-centering: the window reappears wherever it was last placed.
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let core: SharedCore = Arc::new(Core::default());
    core.interval.store(DEFAULT_INTERVAL, Ordering::Relaxed);
    core.remaining.store(DEFAULT_INTERVAL, Ordering::Relaxed);

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostarted"]),
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(core.clone())
        .setup(move |app| {
            let handle = app.handle().clone();

            // Restore saved settings.
            if let Ok(store) = app.store(STORE_FILE) {
                if let Some(v) = store.get("interval").and_then(|v| v.as_u64()) {
                    let v = v.max(1);
                    core.interval.store(v, Ordering::Relaxed);
                    core.remaining.store(v, Ordering::Relaxed);
                }
                if let Some(v) = store.get("zen").and_then(|v| v.as_bool()) {
                    core.zen.store(v, Ordering::Relaxed);
                }
                if let Some(v) = store.get("enabled").and_then(|v| v.as_bool()) {
                    core.enabled.store(v, Ordering::Relaxed);
                }
                if let Some(v) = store.get("start_minimized").and_then(|v| v.as_bool()) {
                    core.start_minimized.store(v, Ordering::Relaxed);
                }
            }

            app.manage(Mutex::new(Labels::default()));

            // System tray. The first item is a (disabled) status line so the
            // current state is visible on right-click.
            let status_i =
                MenuItem::with_id(app, "status", "○ Выключен", false, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Показать окно", true, None::<&str>)?;
            let toggle_i = MenuItem::with_id(app, "toggle", "Включить", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status_i,
                    &PredefinedMenuItem::separator(app)?,
                    &show_i,
                    &toggle_i,
                    &PredefinedMenuItem::separator(app)?,
                    &quit_i,
                ],
            )?;

            let tray_core = core.clone();
            TrayIconBuilder::with_id("main-tray")
                .icon(Image::from_bytes(TRAY_NORMAL)?)
                .tooltip("Cursor Jiggler")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "show" => show_window(app),
                    "toggle" => {
                        let new = !tray_core.enabled.load(Ordering::Relaxed);
                        tray_core.enabled.store(new, Ordering::Relaxed);
                        if new {
                            tray_core.remaining.store(
                                tray_core.interval.load(Ordering::Relaxed),
                                Ordering::Relaxed,
                            );
                        }
                        persist(app, &tray_core);
                        update_status_ui(app, new);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // A single or double left-click on the tray icon opens the window.
                    match event {
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                        | TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => show_window(tray.app_handle()),
                        _ => {}
                    }
                })
                .build(app)?;

            // Keep handles so we can update labels later.
            app.manage(Mutex::new(TrayUi {
                status: status_i,
                toggle: toggle_i,
                show: show_i,
                quit: quit_i,
            }));

            spawn_loop(handle.clone(), core.clone());

            // Reflect any restored "enabled" state in the tray right away.
            update_status_ui(&handle, core.enabled.load(Ordering::Relaxed));

            // The window starts hidden (visible:false in config). Show it now
            // unless we were auto-launched at login with "start minimized" on.
            let autostarted = std::env::args().any(|a| a == "--autostarted");
            let start_min = core.start_minimized.load(Ordering::Relaxed);
            if !(autostarted && start_min) {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it to the tray instead of quitting.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_enabled,
            set_zen,
            set_interval,
            set_start_minimized,
            set_labels
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
