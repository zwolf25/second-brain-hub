mod config;
mod counts;
#[cfg(target_os = "macos")]
mod default_handler;
mod frontmatter;
mod index;
mod logging;
mod render;
mod search;
mod watcher;

use config::AppConfig;
use index::Fields;
use notify_debouncer_mini::Debouncer;
use search::SearchResult;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tantivy::Index as TantivyIndex;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing_appender::non_blocking::WorkerGuard;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase", tag = "state")]
enum IndexStatus {
    Idle { count: usize },
    Indexing,
    Error,
}

struct AppState {
    index_dir: PathBuf,
    config_dir: PathBuf,
    log_dir: PathBuf,
    tantivy: Mutex<Option<(TantivyIndex, Fields)>>,
    config: Mutex<AppConfig>,
    watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    raw_watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    inbox_watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    shared_raw_watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    status: Mutex<IndexStatus>,
    raw_count: Mutex<Option<usize>>,
    inbox_count: Mutex<Option<usize>>,
    shared_raw_count: Mutex<Option<usize>>,
    _log_guard: Mutex<Option<WorkerGuard>>,
}

// A file path the OS handed us at launch (Windows/Linux argv, or a macOS
// open-file event). Deliberately a plain static, not Tauri-managed AppState —
// macOS can deliver the open-file Apple Event before `.setup()` runs (caught
// live: `app.state::<Arc<AppState>>()` here panicked, and since this call
// path crosses tao's ObjC delegate callback, an unwind isn't allowed, so the
// panic became a hard abort/crash instead of a catchable error). A static
// needs no `.manage()` call first, so it's safe at any point in startup.
// Consumed once via `get_launch_file_path`; later opens while already
// running arrive as the "open-file-request" event instead.
static PENDING_OPEN_PATH: Mutex<Option<String>> = Mutex::new(None);

/// Stores a launch-time file path for `get_launch_file_path` to pick up, and
/// also emits it as an event in case the frontend is already listening (the
/// already-running case on every platform, and the rare late-`Opened` case
/// on macOS). A frontend that catches both just re-renders the same doc.
fn handle_open_file(app: &AppHandle, path: String) {
    *PENDING_OPEN_PATH.lock().unwrap() = Some(path.clone());
    let _ = app.emit("open-file-request", path);
}

/// Rebuilds the tantivy index from the vault currently in config, if any is
/// set. Shared by app startup, the "Re-index now" command, and the file
/// watcher's debounced change callback.
fn reindex(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let vault_path = { state.config.lock().unwrap().vault_path.clone() };

    let Some(vault_path) = vault_path else {
        return;
    };

    let set_status = |s: IndexStatus| {
        *state.status.lock().unwrap() = s;
        let _ = app.emit("index-status", s);
    };

    set_status(IndexStatus::Indexing);

    match index::rebuild(&state.index_dir, &vault_path) {
        Ok(count) => match index::open(&state.index_dir) {
            Ok((idx, fields)) => {
                *state.tantivy.lock().unwrap() = Some((idx, fields));
                tracing::info!("indexed {count} docs from {}", vault_path.display());
                set_status(IndexStatus::Idle { count });
            }
            Err(e) => {
                tracing::error!("failed to open index after rebuild: {e}");
                set_status(IndexStatus::Error);
            }
        },
        Err(e) => {
            tracing::error!("failed to rebuild index: {e}");
            set_status(IndexStatus::Error);
        }
    }
}

fn start_watching(app: &AppHandle, vault_path: &std::path::Path) {
    let state = app.state::<Arc<AppState>>();
    let app_for_watcher = app.clone();
    match watcher::start(vault_path, move || reindex(&app_for_watcher)) {
        Ok(debouncer) => *state.watcher.lock().unwrap() = Some(debouncer),
        Err(e) => tracing::warn!("failed to start file watcher: {e}"),
    }
}

/// Recomputes the raw/inbox counts from config and emits the result, shared by
/// startup, the settings-path commands, and each folder's file watcher —
/// same shape as `reindex`/`start_watching` above.
fn recount_raw(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let raw_path = state.config.lock().unwrap().raw_path.clone();
    let count = raw_path.map(|p| counts::count_unprocessed_raw(&p));
    tracing::info!("raw count: {count:?}");
    *state.raw_count.lock().unwrap() = count;
    let _ = app.emit("raw-count-status", count);
}

fn recount_inbox(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let inbox_path = state.config.lock().unwrap().inbox_path.clone();
    let count = inbox_path.map(|p| counts::count_inbox_items(&p));
    tracing::info!("inbox count: {count:?}");
    *state.inbox_count.lock().unwrap() = count;
    let _ = app.emit("inbox-count-status", count);
}

fn start_watching_raw(app: &AppHandle, raw_path: &std::path::Path) {
    let state = app.state::<Arc<AppState>>();
    let app_for_watcher = app.clone();
    match watcher::start(raw_path, move || recount_raw(&app_for_watcher)) {
        Ok(debouncer) => *state.raw_watcher.lock().unwrap() = Some(debouncer),
        Err(e) => tracing::warn!("failed to start raw-folder watcher: {e}"),
    }
}

fn start_watching_inbox(app: &AppHandle, inbox_path: &std::path::Path) {
    let state = app.state::<Arc<AppState>>();
    let app_for_watcher = app.clone();
    match watcher::start(inbox_path, move || recount_inbox(&app_for_watcher)) {
        Ok(debouncer) => *state.inbox_watcher.lock().unwrap() = Some(debouncer),
        Err(e) => tracing::warn!("failed to start inbox-folder watcher: {e}"),
    }
}

/// Same marker-based counting logic as the personal raw badge — it's
/// folder-agnostic, so it works unchanged against the shared vault's own
/// `raw/` folder too.
fn recount_shared_raw(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let shared_raw_path = state.config.lock().unwrap().shared_raw_path.clone();
    let count = shared_raw_path.map(|p| counts::count_unprocessed_raw(&p));
    tracing::info!("shared raw count: {count:?}");
    *state.shared_raw_count.lock().unwrap() = count;
    let _ = app.emit("shared-raw-count-status", count);
}

fn start_watching_shared_raw(app: &AppHandle, shared_raw_path: &std::path::Path) {
    let state = app.state::<Arc<AppState>>();
    let app_for_watcher = app.clone();
    match watcher::start(shared_raw_path, move || recount_shared_raw(&app_for_watcher)) {
        Ok(debouncer) => *state.shared_raw_watcher.lock().unwrap() = Some(debouncer),
        Err(e) => tracing::warn!("failed to start shared-raw-folder watcher: {e}"),
    }
}

#[tauri::command]
fn get_config(state: State<Arc<AppState>>) -> AppConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn autodetect_vault() -> Option<String> {
    config::autodetect_vault().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn set_vault_path(app: AppHandle, state: State<Arc<AppState>>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !config::is_valid_vault(&path) {
        return Err("That folder doesn't contain any .md files.".into());
    }

    {
        let mut cfg = state.config.lock().unwrap();
        cfg.vault_path = Some(path.clone());
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }

    *state.watcher.lock().unwrap() = None; // drop old watcher before starting a new one
    start_watching(&app, &path);
    reindex(&app);
    Ok(())
}

#[tauri::command]
fn reindex_now(app: AppHandle) {
    reindex(&app);
}

#[tauri::command]
fn autodetect_raw() -> Option<String> {
    config::autodetect_raw().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn autodetect_inbox() -> Option<String> {
    config::autodetect_inbox().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn autodetect_shared_raw() -> Option<String> {
    config::autodetect_shared_raw().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn set_raw_path(app: AppHandle, state: State<Arc<AppState>>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.raw_path = Some(path.clone());
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }
    *state.raw_watcher.lock().unwrap() = None;
    start_watching_raw(&app, &path);
    recount_raw(&app);
    Ok(())
}

#[tauri::command]
fn set_inbox_path(app: AppHandle, state: State<Arc<AppState>>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.inbox_path = Some(path.clone());
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }
    *state.inbox_watcher.lock().unwrap() = None;
    start_watching_inbox(&app, &path);
    recount_inbox(&app);
    Ok(())
}

#[tauri::command]
fn set_shared_raw_path(app: AppHandle, state: State<Arc<AppState>>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.shared_raw_path = Some(path.clone());
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }
    *state.shared_raw_watcher.lock().unwrap() = None;
    start_watching_shared_raw(&app, &path);
    recount_shared_raw(&app);
    Ok(())
}

#[tauri::command]
fn get_raw_count(state: State<Arc<AppState>>) -> Option<usize> {
    *state.raw_count.lock().unwrap()
}

#[tauri::command]
fn get_inbox_count(state: State<Arc<AppState>>) -> Option<usize> {
    *state.inbox_count.lock().unwrap()
}

#[tauri::command]
fn get_shared_raw_count(state: State<Arc<AppState>>) -> Option<usize> {
    *state.shared_raw_count.lock().unwrap()
}

#[tauri::command]
fn search_vault(state: State<Arc<AppState>>, query: String) -> Result<Vec<SearchResult>, String> {
    let guard = state.tantivy.lock().unwrap();
    let Some((idx, fields)) = guard.as_ref() else {
        return Ok(vec![]);
    };
    search::search(idx, fields, &query).map_err(|e| e.to_string())
}

#[tauri::command]
fn log_dir_path(state: State<Arc<AppState>>) -> String {
    state.log_dir.to_string_lossy().to_string()
}

#[tauri::command]
fn get_index_status(state: State<Arc<AppState>>) -> IndexStatus {
    *state.status.lock().unwrap()
}

#[tauri::command]
fn render_markdown(path: String) -> Result<render::RenderedDoc, String> {
    render::render_file(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_launch_file_path() -> Option<String> {
    PENDING_OPEN_PATH.lock().unwrap().take()
}

// `None` means "can't tell" (only macOS has a real Launch Services query;
// Windows blocks programmatic default-app changes entirely since Windows 8,
// so there's no equivalent to check) — the frontend uses that to decide
// whether to show the one-click Mac UI or the guided-Settings-link Windows UI.
#[tauri::command]
fn is_default_md_handler() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(default_handler::is_default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[tauri::command]
fn set_default_md_handler() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        default_handler::set_as_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Not supported on this platform — use the Settings link instead.".into())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered first (plugin's own requirement) — handles the
        // Windows/Linux "already running" case: a second launch's argv is
        // forwarded here instead of spawning a second process. macOS routes
        // both cold-start and already-running file-opens through
        // RunEvent::Opened instead, so this is largely a no-op safety net there.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if let Some(path) = argv.get(1) {
                if std::path::Path::new(path).is_file() {
                    handle_open_file(app, path.clone());
                }
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle();
            let config_dir = handle.path().app_config_dir()?;
            let data_dir = handle.path().app_data_dir()?;
            let log_dir = handle.path().app_log_dir()?.join("logs");

            let log_guard = logging::init(&log_dir);

            let config = config::load(&config_dir);
            let index_dir = data_dir.join("index");

            let state = Arc::new(AppState {
                index_dir,
                config_dir,
                log_dir,
                tantivy: Mutex::new(None),
                config: Mutex::new(config.clone()),
                watcher: Mutex::new(None),
                raw_watcher: Mutex::new(None),
                inbox_watcher: Mutex::new(None),
                shared_raw_watcher: Mutex::new(None),
                status: Mutex::new(IndexStatus::Idle { count: 0 }),
                raw_count: Mutex::new(None),
                inbox_count: Mutex::new(None),
                shared_raw_count: Mutex::new(None),
                _log_guard: Mutex::new(Some(log_guard)),
            });
            app.manage(state);

            // Windows/Linux hand a double-clicked file to a freshly-launched
            // process as argv[1] (macOS instead uses RunEvent::Opened, below —
            // args() here is just the app binary path on that platform).
            if let Some(path) = std::env::args().nth(1) {
                if std::path::Path::new(&path).is_file() {
                    handle_open_file(&handle, path);
                }
            }

            if let Some(vault_path) = config.vault_path {
                let app_handle = handle.clone();
                start_watching(&app_handle, &vault_path);
                reindex(&app_handle);
            }
            if let Some(raw_path) = config.raw_path {
                let app_handle = handle.clone();
                start_watching_raw(&app_handle, &raw_path);
                recount_raw(&app_handle);
            }
            if let Some(inbox_path) = config.inbox_path {
                let app_handle = handle.clone();
                start_watching_inbox(&app_handle, &inbox_path);
                recount_inbox(&app_handle);
            }
            if let Some(shared_raw_path) = config.shared_raw_path {
                let app_handle = handle.clone();
                start_watching_shared_raw(&app_handle, &shared_raw_path);
                recount_shared_raw(&app_handle);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            autodetect_vault,
            set_vault_path,
            reindex_now,
            search_vault,
            log_dir_path,
            get_index_status,
            autodetect_raw,
            autodetect_inbox,
            autodetect_shared_raw,
            set_raw_path,
            set_inbox_path,
            set_shared_raw_path,
            get_raw_count,
            get_inbox_count,
            get_shared_raw_count,
            render_markdown,
            get_launch_file_path,
            is_default_md_handler,
            set_default_md_handler,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // macOS open-file/open-url event — covers both a cold launch via
            // "Open With" / default-handler double-click, and a later
            // double-click while already running (macOS routes both through
            // the same running-process Apple Event, no second process).
            if let tauri::RunEvent::Opened { urls } = event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        handle_open_file(app_handle, path.to_string_lossy().to_string());
                    }
                }
            }
        });
}
