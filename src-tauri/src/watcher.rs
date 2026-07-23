use anyhow::Result;
use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use std::path::Path;
use std::time::Duration;

/// Watches the vault folder and invokes `on_change` (debounced ~400ms) after any
/// filesystem event. Caller must keep the returned Debouncer alive for the
/// app's lifetime (e.g. in Tauri managed state) or watching stops.
pub fn start(vault_path: &Path, on_change: impl Fn() + Send + 'static) -> Result<Debouncer<notify::RecommendedWatcher>> {
    let mut debouncer = new_debouncer(Duration::from_millis(400), move |res: DebounceEventResult| {
        match res {
            Ok(events) if !events.is_empty() => on_change(),
            Ok(_) => {}
            Err(e) => tracing::warn!("file watcher error: {e}"),
        }
    })?;

    debouncer
        .watcher()
        .watch(vault_path, RecursiveMode::NonRecursive)?;

    Ok(debouncer)
}
