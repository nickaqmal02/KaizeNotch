// KaizeNotch: the study notch for Windows (and any desktop Tauri runs on).
// A small pill at the top centre of the screen; hover it to open the panel. The tray icon holds the
// report, sharing and quit. The timer itself runs in the page (ui/app.js); Rust owns the files, the
// report, sharing, the window and the tray.

pub mod report;
pub mod share;
pub mod store;

use chrono::Local;
use serde::Serialize;
use share::{Share, View};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use store::{CustomCategory, NewSession, Store, Subject};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

// Pill and panel sizes (logical pixels)
const PILL: (f64, f64) = (220.0, 40.0);
const PANEL_WIDTH: f64 = 560.0;
const TRAY_ID: &str = "kaizenotch";

pub struct App {
    folder: PathBuf,
    share: Option<Arc<Share>>,
}

impl App {
    fn store(&self) -> Store {
        Store::new(&self.folder)
    }
}

// MARK: what the page needs

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryView {
    key: String,
    label: String,
    icon: String,
    color: String,
    built_in: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct State {
    subjects: Vec<Subject>,
    categories: Vec<CategoryView>,
    today_minutes: HashMap<String, f64>,
    today_done: HashSet<String>,
    version: &'static str,
}

fn built_in_categories() -> Vec<CategoryView> {
    [
        ("academic", "book", "#3987e5"),
        ("health", "walk", "#199e70"),
        ("soul", "moon", "#c98500"),
        ("personal", "drop", "#d55181"),
    ]
    .iter()
    .map(|(k, icon, color)| CategoryView {
        key: k.to_string(),
        label: store::built_in_label(k).unwrap().to_string(),
        icon: icon.to_string(),
        color: color.to_string(),
        built_in: true,
    })
    .collect()
}

fn state_of(app: &App) -> State {
    let s = app.store();
    let mut categories = built_in_categories();
    categories.extend(s.load_categories().into_iter().map(|c| CategoryView {
        key: c.key,
        label: c.label,
        icon: c.icon,
        color: c.color,
        built_in: false,
    }));
    let (today_minutes, today_done) = s.today();
    State {
        subjects: s.load_subjects().iter().map(Subject::filled).collect(),
        categories,
        today_minutes,
        today_done,
        version: env!("CARGO_PKG_VERSION"),
    }
}

// After a session or an edit: refresh report.html and (if sharing) the web copy, off the UI thread
fn data_changed(handle: &AppHandle) {
    let app = handle.state::<App>();
    let store = app.store();
    std::thread::spawn(move || {
        let _ = report::write_local(&store);
    });
    if let Some(share) = app.share.clone() {
        if share.is_on() {
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move { upload_with_retry(handle, share).await });
        }
    }
}

async fn upload_with_retry(handle: AppHandle, share: Arc<Share>) {
    if let Some(wait) = share.upload(false).await {
        // too soon after the last upload: one delayed retry, however many changes come in meanwhile
        if share.claim_pending() {
            tokio::time::sleep(wait).await;
            share.release_pending();
            share.upload(false).await;
        }
    }
    refresh_tray(&handle);
}

// MARK: commands

#[tauri::command]
fn get_state(app: tauri::State<App>) -> State {
    state_of(&app)
}

#[tauri::command]
fn save_subjects(
    handle: AppHandle,
    app: tauri::State<App>,
    subjects: Vec<Subject>,
) -> Result<State, String> {
    let mut seen = HashSet::new();
    let clean: Vec<Subject> = subjects
        .into_iter()
        .filter_map(|mut s| {
            s.name = s
                .name
                .trim()
                .chars()
                .take(store::MAX_SUBJECT_NAME)
                .collect();
            s.minutes = s.minutes.clamp(1.0, 180.0);
            s.break_minutes = s.break_minutes.clamp(0.0, 60.0);
            (!s.name.is_empty() && seen.insert(s.name.to_lowercase())).then_some(s)
        })
        .collect();
    app.store()
        .save_subjects(&clean)
        .map_err(|e| e.to_string())?;
    data_changed(&handle);
    Ok(state_of(&app))
}

#[tauri::command]
fn add_category(handle: AppHandle, app: tauri::State<App>, name: String) -> Result<State, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Type a name first".into());
    }
    if name.chars().count() > store::MAX_CATEGORY_LABEL {
        return Err(format!("Keep it to {} letters", store::MAX_CATEGORY_LABEL));
    }
    let s = app.store();
    let mut custom = s.load_categories();
    let all = state_of(&app).categories;
    if all
        .iter()
        .any(|c| c.label.to_lowercase() == name.to_lowercase())
    {
        return Err(format!("{name} is already a tab"));
    }
    let (color, icon) = store::PALETTE[custom.len() % store::PALETTE.len()];
    let key = store::category_key(&name, &all.iter().map(|c| c.key.clone()).collect());
    custom.push(CustomCategory {
        key,
        label: name,
        icon: icon.into(),
        color: color.into(),
    });
    s.save_categories(&custom).map_err(|e| e.to_string())?;
    data_changed(&handle);
    Ok(state_of(&app))
}

// Only a custom category with no activities left; old sessions keep its key, so past time still counts
#[tauri::command]
fn remove_category(
    handle: AppHandle,
    app: tauri::State<App>,
    key: String,
) -> Result<State, String> {
    let s = app.store();
    if s.load_subjects().iter().any(|x| x.category() == key) {
        return Err("Move or delete its activities first".into());
    }
    let custom: Vec<CustomCategory> = s
        .load_categories()
        .into_iter()
        .filter(|c| c.key != key)
        .collect();
    s.save_categories(&custom).map_err(|e| e.to_string())?;
    data_changed(&handle);
    Ok(state_of(&app))
}

#[tauri::command]
fn log_session(
    handle: AppHandle,
    app: tauri::State<App>,
    session: NewSession,
) -> Result<State, String> {
    app.store()
        .append_session(&session)
        .map_err(|e| e.to_string())?;
    if session.actual_min >= 1.0 || session.completed {
        data_changed(&handle); // quick taps don't change the report enough to upload
    }
    Ok(state_of(&app))
}

// The page asks for the panel (with its height) or the pill
#[tauri::command]
fn set_expanded(window: WebviewWindow, expanded: bool, height: Option<f64>) {
    let size = if expanded {
        (PANEL_WIDTH, height.unwrap_or(320.0).clamp(120.0, 640.0))
    } else {
        PILL
    };
    place(&window, size);
}

#[tauri::command]
fn focus_window(window: WebviewWindow) {
    let _ = window.set_focus();
}

#[tauri::command]
fn open_report(handle: AppHandle) {
    open_report_now(&handle);
}

// Sharing, for the panel (the tray menu has the same actions)
fn share_json(app: &App) -> serde_json::Value {
    let Some(share) = &app.share else {
        return serde_json::json!({"available": false, "state": "off"});
    };
    let (state, url, updated) = match share.view() {
        View::Off => ("off", None, None),
        View::Working => ("working", None, None),
        View::On { url, last_upload } => (
            "on",
            Some(url),
            last_upload.map(|t| t.with_timezone(&Local).format("%d %b, %H:%M").to_string()),
        ),
    };
    serde_json::json!({"available": true, "state": state, "url": url, "updated": updated, "error": share.last_error()})
}

#[tauri::command]
fn share_info(app: tauri::State<App>) -> serde_json::Value {
    share_json(&app)
}

#[tauri::command]
async fn share_start(handle: AppHandle) -> serde_json::Value {
    let share = handle.state::<App>().share.clone();
    if let Some(share) = share {
        share.start().await;
    }
    refresh_tray(&handle);
    share_json(&handle.state::<App>())
}

#[tauri::command]
async fn share_stop(handle: AppHandle) -> serde_json::Value {
    let share = handle.state::<App>().share.clone();
    if let Some(share) = share {
        share.stop().await;
    }
    refresh_tray(&handle);
    share_json(&handle.state::<App>())
}

#[tauri::command]
fn share_copy(handle: AppHandle, app: tauri::State<App>) {
    if let Some(url) = app.share.as_ref().and_then(|s| s.link()) {
        let _ = handle.clipboard().write_text(url);
    }
}

#[tauri::command]
fn share_open(handle: AppHandle, app: tauri::State<App>) {
    if let Some(url) = app.share.as_ref().and_then(|s| s.link()) {
        let _ = handle.opener().open_url(url, None::<&str>);
    }
}

// The x on the panel: close KaizeNotch completely (the tray's Quit does the same)
#[tauri::command]
fn quit_app(handle: AppHandle) {
    handle.exit(0);
}

fn open_report_now(handle: &AppHandle) {
    let app = handle.state::<App>();
    if let Ok(path) = report::write_local(&app.store()) {
        let _ = handle
            .opener()
            .open_path(path.to_string_lossy(), None::<&str>);
    }
}

// MARK: window

// Top centre of the screen the window is on, a few pixels below the edge (below the menu bar on a Mac)
fn place(window: &WebviewWindow, (w, h): (f64, f64)) {
    let Ok(Some(monitor)) = window
        .current_monitor()
        .or_else(|_| window.primary_monitor())
    else {
        return;
    };
    let scale = monitor.scale_factor();
    let top = if cfg!(target_os = "macos") { 34.0 } else { 6.0 };
    let x = monitor.position().x + ((monitor.size().width as f64 - w * scale) / 2.0).round() as i32;
    let y = monitor.position().y + (top * scale).round() as i32;
    let _ = window.set_size(LogicalSize::new(w, h));
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

// The page cannot always see the mouse while the window is not focused, so Rust watches the cursor
// and tells it when it enters or leaves the window
fn watch_hover(window: WebviewWindow) {
    std::thread::spawn(move || {
        let mut inside = false;
        loop {
            std::thread::sleep(Duration::from_millis(80));
            let (Ok(c), Ok(p), Ok(s)) = (
                window.cursor_position(),
                window.outer_position(),
                window.outer_size(),
            ) else {
                continue;
            };
            let now = c.x >= p.x as f64
                && c.x < (p.x + s.width as i32) as f64
                && c.y >= p.y as f64
                && c.y < (p.y + s.height as i32) as f64;
            if now != inside {
                inside = now;
                let _ = window.emit_to(window.label(), "hover", inside);
            }
        }
    });
}

// MARK: tray

fn refresh_tray(handle: &AppHandle) {
    if let (Some(tray), Ok(menu)) = (handle.tray_by_id(TRAY_ID), tray_menu(handle)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn tray_menu(handle: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let app = handle.state::<App>();
    let menu = Menu::new(handle)?;
    let note = |text: &str| MenuItem::new(handle, text, false, None::<&str>);
    let item = |id: &str, text: &str| MenuItem::with_id(handle, id, text, true, None::<&str>);
    menu.append(&note(
        "KaizeNotch: hover the pill at the top of the screen",
    )?)?;
    menu.append(&PredefinedMenuItem::separator(handle)?)?;
    menu.append(&item("report", "Open My Report")?)?;
    if let Some(share) = &app.share {
        menu.append(&PredefinedMenuItem::separator(handle)?)?;
        match share.view() {
            View::Off => menu.append(&item("share_start", "Share My Report on the Web…")?)?,
            View::Working => menu.append(&note("Sharing: working…")?)?,
            View::On { url, last_upload } => {
                menu.append(&note("✓ Sharing is on")?)?;
                menu.append(&note(url.trim_start_matches("https://"))?)?;
                if let Some(t) = last_upload {
                    menu.append(&note(&format!(
                        "Updated {}",
                        t.with_timezone(&Local).format("%d %b, %H:%M")
                    ))?)?;
                }
                menu.append(&item("share_copy", "Copy Link")?)?;
                menu.append(&item("share_open", "Open Link in Browser")?)?;
                menu.append(&item("share_stop", "Stop Sharing")?)?;
            }
        }
        if let Some(err) = share.last_error() {
            menu.append(&note(&err)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(handle)?)?;
    let at_login = handle.autolaunch().is_enabled().unwrap_or(false);
    menu.append(&CheckMenuItem::with_id(
        handle,
        "autostart",
        "Start at Login",
        true,
        at_login,
        None::<&str>,
    )?)?;
    menu.append(&item("data", "Show Data Folder")?)?;
    menu.append(&item("quit", "Quit KaizeNotch")?)?;
    Ok(menu)
}

fn on_tray_menu(handle: &AppHandle, id: &str) {
    let app = handle.state::<App>();
    let share = app.share.clone();
    match id {
        "report" => open_report_now(handle),
        "share_start" | "share_stop" => {
            if let Some(share) = share {
                let (h, start) = (handle.clone(), id == "share_start");
                tauri::async_runtime::spawn(async move {
                    if start {
                        share.start().await
                    } else {
                        share.stop().await
                    }
                    refresh_tray(&h);
                });
                std::thread::sleep(Duration::from_millis(30));
                refresh_tray(handle); // shows "working…" at once
            }
        }
        "share_copy" => {
            if let Some(url) = share.and_then(|s| s.link()) {
                let _ = handle.clipboard().write_text(url);
            }
        }
        "share_open" => {
            if let Some(url) = share.and_then(|s| s.link()) {
                let _ = handle.opener().open_url(url, None::<&str>);
            }
        }
        "autostart" => {
            let auto = handle.autolaunch();
            let _ = if auto.is_enabled().unwrap_or(false) {
                auto.disable()
            } else {
                auto.enable()
            };
            refresh_tray(handle);
        }
        "data" => {
            let _ = handle.opener().reveal_item_in_dir(&app.folder);
        }
        "quit" => handle.exit(0),
        _ => {}
    }
}

// MARK: start

pub fn run() {
    let folder = store::default_folder();
    let share = share::api_url().map(|api| Arc::new(Share::new(api, folder.clone())));
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(App { folder, share })
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_subjects,
            add_category,
            remove_category,
            log_session,
            set_expanded,
            focus_window,
            open_report,
            share_info,
            share_start,
            share_stop,
            share_copy,
            share_open,
            quit_app
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory); // no Dock icon
            let handle = app.handle().clone();
            // first run: write the starter activities so both the page and the report see them
            let state = app.state::<App>();
            let s = state.store();
            if !s.subjects_path().exists() {
                let _ = s.save_subjects(&s.load_subjects());
            }
            let _ = report::write_local(&s);

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("KaizeNotch")
                .menu(&tray_menu(&handle)?)
                .show_menu_on_left_click(true)
                .on_menu_event(|h, e| on_tray_menu(h, e.id().as_ref()))
                .build(app)?;

            let window = app.get_webview_window("notch").expect("notch window");
            place(&window, PILL);
            window.show()?;
            watch_hover(window);
            // a share that was on when the app last quit: catch up with anything new
            if state.share.as_ref().is_some_and(|s| s.is_on()) {
                data_changed(&handle);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("KaizeNotch failed to start");
}

// `kaizenotch --report-json [folder] [--shared]` prints the report data and exits (no window)
pub fn report_json_cli(args: &[String]) -> Option<String> {
    if args.get(1).map(String::as_str) != Some("--report-json") {
        return None;
    }
    let folder = args
        .iter()
        .skip(2)
        .find(|a| *a != "--shared")
        .map(PathBuf::from)
        .unwrap_or_else(store::default_folder);
    let shared = args.iter().any(|a| a == "--shared");
    Some(String::from_utf8(report::json_bytes(&Store::new(&folder), shared)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    // Windows' web view keeps old copies of files it has seen under the same address, so an update
    // would show the previous version's page. Every address carries the version to prevent that.
    #[test]
    fn page_addresses_carry_the_app_version() {
        let v = env!("CARGO_PKG_VERSION");
        let html = include_str!("../../ui/index.html");
        assert!(
            html.contains(&format!("style.css?v={v}")),
            "ui/index.html: style.css?v= is not {v}"
        );
        assert!(
            html.contains(&format!("app.js?v={v}")),
            "ui/index.html: app.js?v= is not {v}"
        );
        let conf = include_str!("../tauri.conf.json");
        assert!(
            conf.contains(&format!("\"version\": \"{v}\"")),
            "tauri.conf.json version is not {v}"
        );
        assert!(
            conf.contains(&format!("index.html?v={v}")),
            "tauri.conf.json window url is not index.html?v={v}"
        );
        let pkg = include_str!("../../package.json");
        assert!(
            pkg.contains(&format!("\"version\": \"{v}\"")),
            "package.json version is not {v}"
        );
    }
}
