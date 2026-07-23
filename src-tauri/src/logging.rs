use std::path::Path;
use std::time::{Duration, SystemTime};
use tracing_appender::non_blocking::WorkerGuard;

const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Initializes file logging (rolling daily) and a panic hook that logs crashes
/// to the same file. Returns a guard that must be kept alive for the app's
/// lifetime (held in Tauri managed state) or the background writer thread exits.
pub fn init(log_dir: &Path) -> WorkerGuard {
    std::fs::create_dir_all(log_dir).ok();
    prune_old_logs(log_dir);

    let file_appender = tracing_appender::rolling::daily(log_dir, "second-brain-search.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(false)
        .init();

    std::panic::set_hook(Box::new(|info| {
        tracing::error!("PANIC: {info}");
    }));

    tracing::info!("second-brain-search starting up");
    guard
}

fn prune_old_logs(log_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(log_dir) else { return };
    let cutoff = SystemTime::now().checked_sub(RETENTION);
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else { continue };
        let Ok(modified) = metadata.modified() else { continue };
        if let Some(cutoff) = cutoff {
            if modified < cutoff {
                std::fs::remove_file(entry.path()).ok();
            }
        }
    }
}
