use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};

#[derive(Default)]
pub struct Watches {
    next: AtomicU64,
    live: Mutex<HashMap<String, RecommendedWatcher>>,
}

impl Watches {
    pub fn add(&self, root: &str, emit: impl Fn(Value) + Send + 'static) -> Result<Value, String> {
        let root = crate::repo::resolve(root).map(|r| PathBuf::from(r.checkout))
            .unwrap_or(std::fs::canonicalize(root).map_err(|e| e.to_string())?);
        if !root.is_dir() { return Err("watch root must be a directory".into()); }
        let id = format!("fs-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let event_id = id.clone();
        let event_root = root.clone();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            match result {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    emit(json!({"watch_id": event_id, "root": event_root}));
                }
                Err(error) => emit(json!({"watch_id": event_id, "root": event_root, "error": error.to_string()})),
                _ => {}
            }
        }).map_err(|e| e.to_string())?;
        watcher.watch(&root, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
        // Linked worktrees' index/HEAD live outside the checkout. Watch the
        // shared Git directory too so checkout/commit changes refresh badges.
        if let Some(repo) = crate::repo::resolve(root.to_string_lossy().as_ref()) {
            let git = PathBuf::from(repo.key);
            if !git.starts_with(&root) { watcher.watch(&git, RecursiveMode::Recursive).map_err(|e| e.to_string())?; }
        }
        self.live.lock().map_err(|e| e.to_string())?.insert(id.clone(), watcher);
        Ok(json!({"watch_id": id, "root": root}))
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        // Drop outside the registry lock: native teardown joins a watcher
        // thread and must not block another registration holding that lock.
        let watcher = self.live.lock().map_err(|e| e.to_string())?.remove(id);
        drop(watcher);
        Ok(())
    }

    pub fn clear(&self) {
        if let Ok(mut live) = self.live.lock() {
            let watchers = std::mem::take(&mut *live);
            drop(live);
            drop(watchers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registrations_are_released_and_missing_roots_fail() {
        let root = std::env::temp_dir().join(format!("lazed-watch-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let watches = Watches::default();
        let result = watches.add(root.to_str().unwrap(), |_| {}).unwrap();
        assert_eq!(watches.live.lock().unwrap().len(), 1);
        watches.remove(result["watch_id"].as_str().unwrap()).unwrap();
        watches.remove(result["watch_id"].as_str().unwrap()).unwrap();
        assert!(watches.live.lock().unwrap().is_empty());
        watches.add(root.to_str().unwrap(), |_| {}).unwrap();
        watches.clear();
        assert!(watches.live.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
        assert!(watches.add(root.to_str().unwrap(), |_| {}).is_err());
    }
}
