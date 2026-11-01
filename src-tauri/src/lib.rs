mod audit;
mod categorize;
mod config;
mod counts;
#[cfg(target_os = "macos")]
mod default_handler;
mod frontmatter;
mod graph;
mod index;
mod logging;
mod plugins;
mod render;
mod search;
mod skills;
mod transcripts;
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
    transcripts_watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    transcripts_dir: PathBuf,
    /// Serializes scans: the watcher callback, launch scan and manual rescan can overlap.
    transcripts_scan: Mutex<()>,
    /// Last auto-move failure (e.g. destination missing); cleared by the next success.
    move_error: Mutex<Option<String>>,
    status: Mutex<IndexStatus>,
    raw_count: Mutex<Option<usize>>,
    inbox_count: Mutex<Option<usize>>,
    shared_raw_count: Mutex<Option<usize>>,
    plugin_drift: Mutex<Vec<plugins::PluginDrift>>,
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
    show_main(app);
    *PENDING_OPEN_PATH.lock().unwrap() = Some(path.clone());
    let _ = app.emit("open-file-request", path);
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = (w.show(), w.unminimize(), w.set_focus());
    }
}

/// Tray icon only while "Start at login" is on: closing the window then hides it, so the transcripts
/// watcher, auto rules and log queue keep running. Off: no tray, closing the window quits as before.
fn sync_tray(app: &AppHandle, on: bool) -> tauri::Result<()> {
    use tauri::{menu::{Menu, MenuItem}, tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent}};
    if !on {
        app.remove_tray_by_id("hub");
        return Ok(());
    }
    if app.tray_by_id("hub").is_some() {
        return Ok(());
    }
    let menu = Menu::with_items(
        app,
        &[&MenuItem::with_id(app, "open", "Open Second Brain Hub", true, None::<&str>)?, &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?],
    )?;
    let mut b = TrayIconBuilder::with_id("hub")
        .tooltip("Second Brain Hub")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|t, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(t.app_handle());
            }
        });
    // macOS menu bar: a black "template" icon the system tints for light/dark. Elsewhere the colored app icon.
    #[cfg(target_os = "macos")]
    {
        b = b.icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(i) = app.default_window_icon() {
        b = b.icon(i.clone());
    }
    b.build(app)?;
    Ok(())
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
                // Push a fresh link graph on every reindex (launch, manual, or
                // file-watcher-triggered) — the Vault Neural Map view live-updates
                // off this event instead of needing its own re-open to refresh.
                let _ = app.emit("graph-updated", graph::build_graph(&vault_path));
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

/// Shared vault root, derived from the shared-raw path the same way `check_plugin_drift` does.
fn shared_root(state: &AppState) -> Option<PathBuf> {
    let cfg = state.config.lock().unwrap();
    cfg.shared_raw_path.as_deref().and_then(|p| p.parent()).map(|p| p.to_path_buf())
}

fn load_rules(state: &AppState) -> categorize::Rules {
    categorize::Rules::load(shared_root(state).map(|r| r.join("transcript-audit/categories.json")).as_deref())
}

/// None when the shared vault or this user's inbox slug is missing; corrections still work locally.
fn audit_for(app: &AppHandle, rules_version: u32) -> Option<audit::Audit> {
    let state = app.state::<Arc<AppState>>();
    let slug = config::dirs_home().and_then(|h| std::fs::read_to_string(h.join(".claude/second-brain-inbox/inbox-slug")).ok());
    audit::Audit::new(shared_root(&state).as_deref(), slug, &state.transcripts_dir, &app.package_info().version.to_string(), rules_version)
}

/// Converts/categorizes any new transcripts, logs `auto_other` events, flushes the audit queue,
/// writes the heartbeat, then tells the UI. No-op unless a folder is set.
/// Blocking (file IO); callers off the main thread only.
fn scan_transcripts(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let Some(src) = state.config.lock().unwrap().transcripts_path.clone() else { return };
    let _guard = state.transcripts_scan.lock().unwrap();
    let rules = load_rules(&state);
    match transcripts::scan(&src, &state.transcripts_dir, &rules) {
        Ok(r) => {
            tracing::info!("transcripts scan: {r:?}");
            if let Some(a) = audit_for(app, rules.version) {
                let events: Vec<_> = r.auto_other.iter().map(|e| a.event("auto_other", e, None, None, None)).collect();
                let wrote = match a.queue(&events).and_then(|_| a.flush()) {
                    Ok(n) => n > 0,
                    Err(e) => {
                        tracing::warn!("audit flush failed (queued, will retry): {e:#}");
                        false
                    }
                };
                if let Err(e) = a.heartbeat(wrote) {
                    tracing::warn!("audit heartbeat failed: {e:#}");
                }
            }
        }
        Err(e) => tracing::warn!("transcripts scan failed: {e:#}"),
    }
    run_auto_move(&state);
    sync_raw_stubs(&state);
    let _ = app.emit("transcripts-updated", ());
}

/// Applies the auto rules (caller holds `transcripts_scan`). Failures are kept for the page banner and retried next scan.
fn run_auto_move(state: &AppState) {
    let rules = state.config.lock().unwrap().auto_rules.clone();
    *state.move_error.lock().unwrap() = if rules.iter().any(|r| r.enabled && r.action == "move") {
        match transcripts::auto_move(&state.transcripts_dir, &rules) {
            Ok(r) if !r.errors.is_empty() => Some(r.errors.join("; ")),
            Ok(r) => (r.skipped > 0).then(|| format!("{} transcript(s) could not be moved; will retry", r.skipped)),
            Err(e) => Some(format!("{e:#}")),
        }
    } else {
        None
    };
}

/// Writes raw-note stubs for the auto "Add to Second Brain" rules, applies the auto-hide rules, then refreshes the VoC hand-off file.
/// Caller holds `transcripts_scan`. Failures only log; the next scan retries. No Claude task is involved.
fn sync_raw_stubs(state: &AppState) {
    let (rules, raw) = {
        let cfg = state.config.lock().unwrap();
        (cfg.auto_rules.clone(), cfg.raw_path.clone())
    };
    if let Some(raw) = raw {
        if let Err(e) = transcripts::auto_add_to_raw(&state.transcripts_dir, &raw, &rules) {
            tracing::warn!("auto raw notes: {e:#}");
        }
    }
    if let Err(e) = transcripts::auto_hide(&state.transcripts_dir, &rules) {
        tracing::warn!("auto hide: {e:#}");
    }
    let Some(bridge) = transcripts::bridge_dir() else { return };
    let _ = std::fs::remove_file(bridge.join("log-queue.json")); // left by 0.7.x; nothing reads it any more
    if rules.iter().any(|r| r.action == "voc") || bridge.join("voc-queue.json").exists() {
        if let Err(e) = transcripts::write_voc_queue(&state.transcripts_dir, &rules, &bridge) {
            tracing::warn!("write voc queue: {e:#}");
        }
    }
}

fn start_watching_transcripts(app: &AppHandle, path: &std::path::Path) {
    let state = app.state::<Arc<AppState>>();
    let app_for_watcher = app.clone();
    match watcher::start(path, move || scan_transcripts(&app_for_watcher)) {
        Ok(debouncer) => *state.transcripts_watcher.lock().unwrap() = Some(debouncer),
        Err(e) => tracing::warn!("failed to start transcripts-folder watcher: {e}"),
    }
}

/// Start at login defaults to on, once, for anyone using transcripts (the watcher is its only purpose).
/// Turning it off later sticks: the flag is never reset.
fn default_autostart(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let state = app.state::<Arc<AppState>>();
    let mut cfg = state.config.lock().unwrap();
    if cfg.autostart_default_applied {
        return;
    }
    if app.autolaunch().enable().is_ok() {
        let _ = sync_tray(app, true);
        cfg.autostart_default_applied = true;
        let _ = config::save(&state.config_dir, &cfg);
    }
}

/// Starts the watcher and a full scan (new files since last launch) off the UI thread.
fn begin_transcripts(app: &AppHandle, path: &std::path::Path) {
    default_autostart(app);
    start_watching_transcripts(app, path);
    let app = app.clone();
    std::thread::spawn(move || scan_transcripts(&app));
}

/// Checked at launch and on Refresh (`refresh_plugin_drift`) — no watcher here
/// (unlike the raw/inbox counts).
/// Shared vault root is the shared-raw folder's parent, since
/// `autodetect_shared_raw` points that path at `<root>/raw`.
fn check_plugin_drift(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let shared_raw_path = state.config.lock().unwrap().shared_raw_path.clone();
    let Some(root) = shared_raw_path.as_deref().and_then(|p| p.parent()) else {
        return;
    };
    let Some(home) = config::dirs_home() else {
        return;
    };
    let installed_path = home.join(".claude/plugins/installed_plugins.json");
    let drift = plugins::check_drift(root, &installed_path);
    tracing::info!("plugin drift: {} plugin(s) behind", drift.len());
    *state.plugin_drift.lock().unwrap() = drift;
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
fn refresh_plugin_drift(app: AppHandle) -> Vec<plugins::PluginDrift> {
    check_plugin_drift(&app);
    app.state::<Arc<AppState>>().plugin_drift.lock().unwrap().clone()
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
    check_plugin_drift(&app);
    Ok(())
}

#[tauri::command]
fn autodetect_transcripts() -> Option<String> {
    transcripts::autodetect_folder().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn set_transcripts_path(app: AppHandle, state: State<Arc<AppState>>, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err("That folder doesn't exist.".into());
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.transcripts_path = Some(path.clone());
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }
    *state.transcripts_watcher.lock().unwrap() = None;
    begin_transcripts(&app, &path);
    Ok(())
}

/// "Stop syncing": halts the watcher; converted files and state stay, so re-enabling reuses them.
#[tauri::command]
fn stop_transcripts(state: State<Arc<AppState>>) -> Result<(), String> {
    *state.transcripts_watcher.lock().unwrap() = None;
    let mut cfg = state.config.lock().unwrap();
    cfg.transcripts_path = None;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn dismiss_transcripts(state: State<Arc<AppState>>) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.transcripts_dismissed = true;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_show_transcripts_page(state: State<Arc<AppState>>, show: bool) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.hide_transcripts_page = !show;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn rescan_transcripts(app: AppHandle) {
    std::thread::spawn(move || scan_transcripts(&app));
}

#[tauri::command]
fn get_transcript_categories(state: State<Arc<AppState>>) -> Vec<String> {
    let mut cats = load_rules(&state).categories;
    cats.sort_by_key(|c| c.to_lowercase());
    cats
}

/// Manual correction for one or many transcripts. Always applied locally (and remembered for
/// the series); the audit event is queued and flushed when the shared drive is reachable.
#[tauri::command]
fn set_transcript_category(app: AppHandle, state: State<Arc<AppState>>, vtts: Vec<String>, category: String) -> Result<(), String> {
    let rules = load_rules(&state);
    if !rules.categories.contains(&category) {
        return Err(format!("Unknown category: {category}"));
    }
    let _guard = state.transcripts_scan.lock().unwrap();
    let changes = transcripts::set_category(&state.transcripts_dir, &vtts, &category).map_err(|e| e.to_string())?;
    if let Some(a) = audit_for(&app, rules.version) {
        let events: Vec<_> = changes
            .iter()
            .filter(|c| c.from != category)
            .map(|c| a.event("manual_change", &c.entry, Some(&c.from), Some(&category), Some(&c.auto_source)))
            .collect();
        let wrote = a.queue(&events).and_then(|_| a.flush()).map(|n| n > 0).unwrap_or_else(|e| {
            tracing::warn!("audit flush failed (queued, will retry): {e:#}");
            false
        });
        let _ = a.heartbeat(wrote);
    }
    Ok(())
}

#[tauri::command]
fn get_audit_status(app: AppHandle) -> audit::Status {
    let rules_version = load_rules(&app.state::<Arc<AppState>>()).version;
    audit_for(&app, rules_version).map(|a| a.status()).unwrap_or_else(audit::disabled_status)
}

#[tauri::command]
fn set_transcripts_hidden(state: State<Arc<AppState>>, vtts: Vec<String>, hidden: bool) -> Result<(), String> {
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::set_hidden(&state.transcripts_dir, &vtts, hidden).map_err(|e| e.to_string())
}

/// "Add to Second Brain": writes a raw-note stub per transcript and hides it. Returns (written, skipped).
#[tauri::command]
fn add_transcripts_to_raw(state: State<Arc<AppState>>, vtts: Vec<String>) -> Result<(usize, usize), String> {
    let raw = state.config.lock().unwrap().raw_path.clone().ok_or("Choose your raw folder in Settings first.")?;
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::add_to_raw(&state.transcripts_dir, &raw, &vtts).map_err(|e| format!("{e:#}"))
}

/// Transcripts an enabled VoC rule has flagged, for the "ready for VoC" button.
#[tauri::command]
fn get_voc_ready(state: State<Arc<AppState>>) -> Vec<String> {
    let rules = state.config.lock().unwrap().auto_rules.clone();
    transcripts::voc_ready(&state.transcripts_dir, &rules)
}

/// The one-time "add future transcripts automatically?" offer was answered.
#[tauri::command]
fn dismiss_raw_stub_offer(state: State<Arc<AppState>>) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.raw_stub_offer_dismissed = true;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())
}

/// Called after the VoC deep link opens; badge only.
/// Recorded once the user confirms the automatic-VoC consent screen.
#[tauri::command]
fn set_voc_consent(state: State<Arc<AppState>>, on: bool) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.voc_consent = on;
    config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())
}

/// The one category whose transcripts offer VoC actions.
#[tauri::command]
fn get_voc_category(state: State<Arc<AppState>>) -> String {
    load_rules(&state).voc_category
}

#[tauri::command]
fn mark_transcripts_voc(state: State<Arc<AppState>>, vtts: Vec<String>) -> Result<(), String> {
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::mark_voc(&state.transcripts_dir, &vtts).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_transcripts_voc_skip(state: State<Arc<AppState>>, vtts: Vec<String>, skip: bool) -> Result<(), String> {
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::set_voc_skip(&state.transcripts_dir, &vtts, skip).map_err(|e| e.to_string())
}

#[tauri::command]
fn undo_move_transcripts(state: State<Arc<AppState>>, vtts: Vec<String>) -> Result<transcripts::MoveReport, String> {
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::undo_move(&state.transcripts_dir, &vtts).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn move_transcripts(state: State<Arc<AppState>>, vtts: Vec<String>, dest: String) -> Result<transcripts::MoveReport, String> {
    let _guard = state.transcripts_scan.lock().unwrap();
    transcripts::move_selected(&state.transcripts_dir, &vtts, std::path::Path::new(&dest)).map_err(|e| format!("{e:#}"))
}

/// Saves the whole ordered rule list and applies it once right away.
#[tauri::command]
fn set_auto_rules(app: AppHandle, state: State<Arc<AppState>>, rules: Vec<config::AutoRule>) -> Result<(), String> {
    let no_raw = state.config.lock().unwrap().raw_path.is_none();
    if let Some(bad) = rules.iter().find(|r| {
        !["move", "log-it", "voc", "hide"].contains(&r.action.as_str()) || (r.action == "move" && r.dest.is_none()) || (r.action == "log-it" && r.enabled && no_raw)
    }) {
        return Err(format!("Rule {} is incomplete (a move rule needs a folder; \"Add to Second Brain\" needs your raw folder set in Settings).", bad.id));
    }
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.auto_rules = rules;
        config::save(&state.config_dir, &cfg).map_err(|e| e.to_string())?;
    }
    std::thread::spawn(move || {
        let state = app.state::<Arc<AppState>>();
        let _guard = state.transcripts_scan.lock().unwrap();
        run_auto_move(&state);
        sync_raw_stubs(&state);
        let _ = app.emit("transcripts-updated", ());
    });
    Ok(())
}

#[tauri::command]
fn preview_auto_rule(state: State<Arc<AppState>>, rules: Vec<config::AutoRule>, idx: usize) -> usize {
    transcripts::auto_move_preview(&state.transcripts_dir, &rules, idx)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Start at login so the transcripts watcher keeps running. Off unless the user turns it on.
#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let l = app.autolaunch();
    (if on { l.enable() } else { l.disable() }).map_err(|e| e.to_string())?;
    sync_tray(&app, on).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_move_error(state: State<Arc<AppState>>) -> Option<String> {
    state.move_error.lock().unwrap().clone()
}

#[tauri::command]
fn get_transcripts(state: State<Arc<AppState>>) -> Vec<transcripts::Row> {
    transcripts::rows(&state.transcripts_dir)
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
fn get_plugin_drift(state: State<Arc<AppState>>) -> Vec<plugins::PluginDrift> {
    state.plugin_drift.lock().unwrap().clone()
}

/// Resolved fresh on every call (same shared-vault-root logic as
/// `check_plugin_drift`) — a ~150-file walk is cheap enough that caching
/// would only risk staleness for no measurable benefit.
#[tauri::command]
fn get_skills(state: State<Arc<AppState>>) -> Vec<skills::SkillInfo> {
    let shared_raw_path = state.config.lock().unwrap().shared_raw_path.clone();
    let Some(root) = shared_raw_path.as_deref().and_then(|p| p.parent()) else {
        return vec![];
    };
    skills::list_skills(root)
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
fn get_vault_graph(state: State<Arc<AppState>>) -> graph::GraphData {
    let vault_path = state.config.lock().unwrap().vault_path.clone();
    match vault_path {
        Some(p) => graph::build_graph(&p),
        None => graph::GraphData::default(),
    }
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

// Any local markdown file may be edited (user decision); the Edit button is only
// offered when the file is writable. `render::*_for_edit` enforce the .md extension.
#[tauri::command]
fn can_edit_markdown(path: String) -> bool {
    std::fs::metadata(&path).map(|m| m.is_file() && !m.permissions().readonly()).unwrap_or(false)
}

#[tauri::command]
fn read_for_edit(path: String) -> Result<render::EditDoc, String> {
    render::read_for_edit(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_for_edit(path: String, header: String, body: String) -> Result<(), String> {
    render::write_for_edit(std::path::Path::new(&path), &header, &body).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_launch_file_path()-> Option<String> {
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
            show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|w, e| {
            use tauri_plugin_autostart::ManagerExt;
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "main" && w.app_handle().autolaunch().is_enabled().unwrap_or(false) {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .setup(|app| {
            let handle = app.handle();
            {
                use tauri_plugin_autostart::ManagerExt;
                // The window starts hidden (tauri.conf.json); only a login launch (`--hidden`) keeps it that way.
                if !std::env::args().any(|a| a == "--hidden") {
                    show_main(handle);
                }
                let _ = sync_tray(handle, handle.autolaunch().is_enabled().unwrap_or(false));
            }
            let config_dir = handle.path().app_config_dir()?;
            let data_dir = handle.path().app_data_dir()?;
            let log_dir = handle.path().app_log_dir()?.join("logs");

            let log_guard = logging::init(&log_dir);

            let config = config::load(&config_dir);
            let index_dir = data_dir.join("index");
            // The AI chat (v2) was removed in v3 but left ~430 MB of downloaded models behind; nothing reads this folder.
            let _ = std::fs::remove_dir_all(data_dir.join("models"));

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
                transcripts_watcher: Mutex::new(None),
                transcripts_dir: data_dir.join("transcripts"),
                transcripts_scan: Mutex::new(()),
                move_error: Mutex::new(None),
                status: Mutex::new(IndexStatus::Idle { count: 0 }),
                raw_count: Mutex::new(None),
                inbox_count: Mutex::new(None),
                shared_raw_count: Mutex::new(None),
                plugin_drift: Mutex::new(Vec::new()),
                _log_guard: Mutex::new(Some(log_guard)),
            });
            app.manage(state);

            // Windows/Linux hand a double-clicked file to a freshly-launched
            // process as argv[1] (macOS instead uses RunEvent::Opened, below —
            // args() here is just the app binary path on that platform).
            if let Some(path) = std::env::args().nth(1) {
                if std::path::Path::new(&path).is_file() {
                    handle_open_file(handle, path);
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
            if let Some(path) = config.transcripts_path {
                begin_transcripts(handle, &path);
            }
            check_plugin_drift(handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            autodetect_vault,
            set_vault_path,
            reindex_now,
            search_vault,
            get_vault_graph,
            log_dir_path,
            get_index_status,
            autodetect_raw,
            autodetect_inbox,
            autodetect_shared_raw,
            set_raw_path,
            set_inbox_path,
            set_shared_raw_path,
            autodetect_transcripts,
            set_transcripts_path,
            stop_transcripts,
            dismiss_transcripts,
            set_show_transcripts_page,
            rescan_transcripts,
            get_transcripts,
            set_transcripts_hidden,
            move_transcripts,
            undo_move_transcripts,
            set_auto_rules,
            preview_auto_rule,
            get_move_error,
            get_autostart,
            set_autostart,
            add_transcripts_to_raw,
            get_voc_ready,
            dismiss_raw_stub_offer,
            get_transcript_categories,
            get_voc_category,
            set_voc_consent,
            mark_transcripts_voc,
            set_transcripts_voc_skip,
            set_transcript_category,
            get_audit_status,
            get_raw_count,
            get_inbox_count,
            get_shared_raw_count,
            get_plugin_drift,
            refresh_plugin_drift,
            get_skills,
            render_markdown,
            can_edit_markdown,
            read_for_edit,
            write_for_edit,
            get_launch_file_path,
            is_default_md_handler,
            set_default_md_handler,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, _event| {
            // macOS open-file/open-url event — covers both a cold launch via
            // "Open With" / default-handler double-click, and a later
            // double-click while already running (macOS routes both through
            // the same running-process Apple Event, no second process).
            // `RunEvent::Opened` only exists on macOS/iOS — Windows/Linux rely
            // entirely on argv (`.setup()`'s cold-start check + the
            // single-instance plugin's forwarded argv) instead.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = _event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        handle_open_file(_app_handle, path.to_string_lossy().to_string());
                    }
                }
            }
        });
}
