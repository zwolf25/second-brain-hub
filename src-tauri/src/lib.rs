mod config;
mod frontmatter;
mod index;
mod llm;
mod logging;
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
    models_dir: PathBuf,
    tantivy: Mutex<Option<(TantivyIndex, Fields)>>,
    config: Mutex<AppConfig>,
    watcher: Mutex<Option<Debouncer<notify::RecommendedWatcher>>>,
    status: Mutex<IndexStatus>,
    embedding_model: Mutex<Option<llm::EmbeddingModel>>,
    chunk_embeddings: Mutex<Vec<llm::ChunkEmbedding>>,
    _log_guard: Mutex<Option<WorkerGuard>>,
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

    // Only refresh chunk embeddings if chat has already been enabled this
    // session — most collaborators who never open the Chat tab pay nothing.
    let embedding_guard = state.embedding_model.lock().unwrap();
    if let Some(embed_model) = embedding_guard.as_ref() {
        match llm::build_chunk_embeddings(&vault_path, embed_model) {
            Ok(chunks) => {
                tracing::info!("refreshed {} chat chunk embeddings", chunks.len());
                *state.chunk_embeddings.lock().unwrap() = chunks;
            }
            Err(e) => tracing::error!("failed to refresh chunk embeddings: {e}"),
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

#[derive(Serialize)]
struct ChatAvailability {
    embedding_ready: bool,
}

#[tauri::command]
fn chat_availability(state: State<Arc<AppState>>) -> ChatAvailability {
    ChatAvailability { embedding_ready: state.embedding_model.lock().unwrap().is_some() }
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    model: String,
    downloaded: u64,
    total: u64,
}

fn progress_emitter(app: &AppHandle) -> impl Fn(&str, u64, u64) + '_ {
    move |model, downloaded, total| {
        let _ = app.emit(
            "model-download-progress",
            DownloadProgress { model: model.to_string(), downloaded, total },
        );
    }
}

/// Downloads (if needed) and loads the embedding model, then builds chunk
/// embeddings for the current vault. Idempotent — safe to call every time the
/// Chat tab opens.
#[tauri::command]
fn enable_chat(app: AppHandle, state: State<Arc<AppState>>) -> Result<usize, String> {
    if state.embedding_model.lock().unwrap().is_none() {
        let paths = llm::ensure_embedding_files(&progress_emitter(&app), &state.models_dir)
            .map_err(|e| e.to_string())?;
        let _ = app.emit("chat-status", "Loading embedding model…");
        let model = llm::EmbeddingModel::load(&paths).map_err(|e| e.to_string())?;
        *state.embedding_model.lock().unwrap() = Some(model);
    }

    let _ = app.emit("chat-status", "Building search index for chat (one-time, may take a moment)…");

    let vault_path = state
        .config
        .lock()
        .unwrap()
        .vault_path
        .clone()
        .ok_or("No vault configured yet.")?;

    let guard = state.embedding_model.lock().unwrap();
    let model = guard.as_ref().unwrap();
    let chunks = llm::build_chunk_embeddings(&vault_path, model).map_err(|e| e.to_string())?;
    let count = chunks.len();
    *state.chunk_embeddings.lock().unwrap() = chunks;
    Ok(count)
}

#[derive(Serialize)]
struct RelatedResult {
    path: String,
    title: String,
    snippet: String,
    score: f32,
}

#[tauri::command]
fn related_docs(state: State<Arc<AppState>>, query: String) -> Result<Vec<RelatedResult>, String> {
    let guard = state.embedding_model.lock().unwrap();
    let model = guard.as_ref().ok_or("Chat isn't enabled yet.")?;
    let chunks = state.chunk_embeddings.lock().unwrap();
    let top = llm::top_related(model, &chunks, &query, 8).map_err(|e| e.to_string())?;
    Ok(top
        .into_iter()
        .map(|(score, c)| RelatedResult {
            path: c.path,
            title: c.title,
            snippet: c.text.chars().take(240).collect(),
            score,
        })
        .collect())
}

/// Retrieves context via the already-loaded embedding model, then loads the
/// chat model fresh for this one request and drops it — no standing cost.
#[tauri::command]
fn write_answer(app: AppHandle, state: State<Arc<AppState>>, query: String) -> Result<String, String> {
    let top = {
        let guard = state.embedding_model.lock().unwrap();
        let model = guard.as_ref().ok_or("Chat isn't enabled yet.")?;
        let chunks = state.chunk_embeddings.lock().unwrap();
        llm::top_related(model, &chunks, &query, 3).map_err(|e| e.to_string())?
    };
    let chat_paths = llm::ensure_chat_files(&progress_emitter(&app), &state.models_dir)
        .map_err(|e| e.to_string())?;
    llm::generate_answer(&chat_paths, &query, &top).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle();
            let config_dir = handle.path().app_config_dir()?;
            let data_dir = handle.path().app_data_dir()?;
            let log_dir = handle.path().app_log_dir()?.join("logs");

            let log_guard = logging::init(&log_dir);

            let config = config::load(&config_dir);
            let index_dir = data_dir.join("index");
            let models_dir = data_dir.join("models");

            let state = Arc::new(AppState {
                index_dir,
                config_dir,
                log_dir,
                models_dir,
                tantivy: Mutex::new(None),
                config: Mutex::new(config.clone()),
                watcher: Mutex::new(None),
                status: Mutex::new(IndexStatus::Idle { count: 0 }),
                embedding_model: Mutex::new(None),
                chunk_embeddings: Mutex::new(Vec::new()),
                _log_guard: Mutex::new(Some(log_guard)),
            });
            app.manage(state);

            if let Some(vault_path) = config.vault_path {
                let app_handle = handle.clone();
                start_watching(&app_handle, &vault_path);
                reindex(&app_handle);
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
            chat_availability,
            enable_chat,
            related_docs,
            write_answer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
