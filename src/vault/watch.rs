//! The file watcher. It does not refresh anything itself: it only marks the
//! part of the cache that a change on disk affects, and the next call that
//! needs that part rescans it.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::{DAILY_DIR, visibility::ignored_by_name};

/// Set by the file watcher when something on disk changed.
#[derive(Default)]
pub(super) struct Dirty {
    /// A visible change outside the Daily directory.
    pub(super) notes: AtomicBool,
    /// A change inside the Daily directory.
    pub(super) daily: AtomicBool,
}

impl Dirty {
    fn mark_all(&self) {
        self.notes.store(true, Ordering::SeqCst);
        self.daily.store(true, Ordering::SeqCst);
    }
}

/// Watch the notebook and flag the affected part of the cache on any change.
pub(super) fn start_watcher(root: &Path, dirty: Arc<Dirty>) -> notify::Result<RecommendedWatcher> {
    let watched = root.to_path_buf();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
            Ok(event) if event.need_rescan() || event.paths.is_empty() => dirty.mark_all(),
            Ok(event) => {
                for path in &event.paths {
                    mark_changed(&watched, path, &dirty);
                }
            }
            Err(_) => dirty.mark_all(),
        })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

fn mark_changed(root: &Path, path: &Path, dirty: &Dirty) {
    let Ok(rel) = path.strip_prefix(root) else {
        return dirty.mark_all();
    };
    let names: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    let Some((_, parents)) = names.split_last() else {
        return dirty.mark_all();
    };
    if names[0].eq_ignore_ascii_case(DAILY_DIR) {
        dirty.daily.store(true, Ordering::SeqCst);
    } else if !parents.iter().any(|name| ignored_by_name(name, true)) {
        // Churn inside ignored directories (.git, .obsidian) changes nothing
        // the tools can see.
        dirty.notes.store(true, Ordering::SeqCst);
    }
}
