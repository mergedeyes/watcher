//! Watching the object files for changes.

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

/// Changes are collected this long before a reload, so an editor saving
/// several files (or one file in steps) causes a single reload.
const SETTLE: Duration = Duration::from_millis(500);

/// Sends on `changed` when `*.toml` files below `dir` change. Watching
/// stops when the returned watcher is dropped.
pub fn watch(dir: &Path, changed: mpsc::Sender<()>) -> notify::Result<RecommendedWatcher> {
    let (raw_tx, mut raw_rx) = mpsc::channel(1);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && relevant(&event)
        {
            // Full means a notification is already pending.
            let _ = raw_tx.try_send(());
        }
    })?;
    watcher.watch(dir, RecursiveMode::Recursive)?;
    tokio::spawn(async move {
        while raw_rx.recv().await.is_some() {
            tokio::time::sleep(SETTLE).await;
            while raw_rx.try_recv().is_ok() {}
            if changed.send(()).await.is_err() {
                break;
            }
        }
    });
    Ok(watcher)
}

fn relevant(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event.paths.iter().any(|p| {
        p.extension().is_some_and(|e| e == "toml")
            && !p
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
    })
}
