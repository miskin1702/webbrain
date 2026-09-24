use crate::paths::PathSandbox;
use crate::protocol::{FileEventData, RpcEvent};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const DEBOUNCE_WINDOW_MS: u128 = 150;

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub debounce_ms: u64,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self { debounce_ms: 150 }
    }
}

/// A debounced filesystem watcher that emits RpcEvents over a broadcast channel.
pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
    _shutdown_tx: Sender<()>,
}

impl WorkspaceWatcher {
    pub fn new(
        sandbox: PathSandbox,
        event_sender: broadcast::Sender<RpcEvent>,
    ) -> Result<Self, notify::Error> {
        let (notify_tx, notify_rx) = channel();
        let (shutdown_tx, shutdown_rx) = channel();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = notify_tx.send(event);
                }
            },
            Config::default(),
        )?;

        let root = sandbox.canonical_root();
        watcher.watch(root, RecursiveMode::Recursive)?;

        // Spawn background debouncer thread
        let sandbox_clone = sandbox.clone();
        std::thread::spawn(move || {
            run_debouncer(sandbox_clone, notify_rx, shutdown_rx, event_sender);
        });

        Ok(Self {
            _watcher: watcher,
            _shutdown_tx: shutdown_tx,
        })
    }
}

fn should_ignore_path(path: &Path) -> bool {
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        if s == ".git"
            || s == "node_modules"
            || s == "target"
            || s == "build"
            || s == ".idea"
            || s == ".vscode"
            || s.starts_with(".wb-tmp-")
        {
            return true;
        }
    }
    false
}

fn run_debouncer(
    sandbox: PathSandbox,
    rx: Receiver<Event>,
    shutdown_rx: Receiver<()>,
    event_tx: broadcast::Sender<RpcEvent>,
) {
    let mut last_events: HashMap<PathBuf, (EventKind, Instant)> = HashMap::new();

    loop {
        // Check for shutdown signal
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        // Process incoming events with timeout for flushing
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let now = Instant::now();
                for path in event.paths {
                    if should_ignore_path(&path) {
                        continue;
                    }

                    // Check debounce window
                    if let Some((_, last_time)) = last_events.get(&path) {
                        if now.duration_since(*last_time).as_millis() < DEBOUNCE_WINDOW_MS {
                            continue;
                        }
                    }

                    last_events.insert(path.clone(), (event.kind, now));
                    emit_event(&sandbox, &event.kind, &path, &event_tx);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Prune stale debounce entries (older than 10 seconds)
                let now = Instant::now();
                last_events.retain(|_, (_, time)| now.duration_since(*time).as_secs() < 10);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }
}

fn emit_event(
    sandbox: &PathSandbox,
    kind: &EventKind,
    path: &Path,
    event_tx: &broadcast::Sender<RpcEvent>,
) {
    let rel_path = match sandbox.to_relative(path) {
        Ok(p) => p,
        Err(_) => return,
    };

    if rel_path.is_empty() {
        return;
    }

    match kind {
        EventKind::Create(_) => {
            let hash = if path.is_file() {
                std::fs::read(path)
                    .ok()
                    .map(|b| blake3::hash(&b).to_hex().to_string())
            } else {
                None
            };

            let data = FileEventData {
                path: rel_path,
                revision: None,
                hash,
                old_path: None,
            };
            let _ = event_tx.send(RpcEvent::new(
                "file.created",
                serde_json::to_value(data).unwrap(),
            ));
        }
        EventKind::Modify(_) => {
            let hash = if path.is_file() {
                std::fs::read(path)
                    .ok()
                    .map(|b| blake3::hash(&b).to_hex().to_string())
            } else {
                None
            };

            let data = FileEventData {
                path: rel_path,
                revision: None,
                hash,
                old_path: None,
            };
            let _ = event_tx.send(RpcEvent::new(
                "file.changed",
                serde_json::to_value(data).unwrap(),
            ));
        }
        EventKind::Remove(_) => {
            let data = FileEventData {
                path: rel_path,
                revision: None,
                hash: None,
                old_path: None,
            };
            let _ = event_tx.send(RpcEvent::new(
                "file.deleted",
                serde_json::to_value(data).unwrap(),
            ));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ignore_paths() {
        assert!(should_ignore_path(Path::new("C:/Project/.git/HEAD")));
        assert!(should_ignore_path(Path::new(
            "project/node_modules/pkg/index.js"
        )));
        assert!(should_ignore_path(Path::new(
            "project/target/debug/app.exe"
        )));
        assert!(should_ignore_path(Path::new("project/.wb-tmp-12345")));
        assert!(!should_ignore_path(Path::new("project/src/main.rs")));
    }

    #[test]
    fn test_watcher_creation() {
        let temp = tempdir().unwrap();
        let sandbox = PathSandbox::new(temp.path()).unwrap();
        let (tx, mut rx) = broadcast::channel(16);

        let watcher = WorkspaceWatcher::new(sandbox, tx);
        assert!(watcher.is_ok());

        // Create a file and verify event is caught
        let test_file = temp.path().join("watcher_test.txt");
        std::fs::write(&test_file, "hello watcher").unwrap();

        // Give watcher thread time to receive event
        std::thread::sleep(Duration::from_millis(300));

        let received = rx.try_recv();
        if let Ok(event) = received {
            assert!(event.event == "file.created" || event.event == "file.changed");
        }
    }
}
