//! What is kept in memory: the whole notebook tree (without Daily) and
//! today's folder, including the whole text of its Summary. Everything else
//! is read from disk on each call.

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use chrono::NaiveDate;

use super::{
    Result, Vault,
    daily::{Day, stamp},
    entries::{Node, find, scan_dir},
    paths::{Resolved, display_or_root},
};

/// The watcher is the normal way the cache is refreshed. Watchers can lose
/// events, so anything cached is also read again once it is this old.
const MAX_CACHE_AGE: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct Cache {
    /// Every file and folder of the notebook (without Daily), and when it
    /// was scanned.
    notes: Option<(Arc<Vec<Node>>, Instant)>,
    /// Today's daily folder, and when it was loaded.
    today: Option<(Arc<Day>, Instant)>,
}

impl Vault {
    /// The entries of a resolved directory and, for a date, the day they
    /// belong to. A tool call gets both from one look at the cache or disk.
    pub(super) fn contents(&self, target: &Resolved) -> Result<(Vec<Node>, Option<Arc<Day>>)> {
        let missing = || format!("Not found: {}", display_or_root(target));
        // `resolve` found the directory on disk. If the cache does not have
        // it, the watcher has not caught up yet: rescan once.
        match target.date {
            Some(date) => {
                let lookup = |day: &Day| Some(find(day.nodes()?, &target.rel)?.to_vec());
                let mut day = self.day(date);
                if lookup(&day).is_none() {
                    self.dirty.daily.store(true, Ordering::SeqCst);
                    day = self.day(date);
                }
                let nodes = lookup(&day).ok_or_else(missing)?;
                Ok((nodes, Some(day)))
            }
            None => {
                let mut notes = self.notes();
                if find(&notes, &target.rel).is_none() {
                    self.dirty.notes.store(true, Ordering::SeqCst);
                    notes = self.notes();
                }
                let nodes = find(&notes, &target.rel).ok_or_else(missing)?;
                Ok((nodes.to_vec(), None))
            }
        }
    }

    /// A day's folder: from the cache for today, from disk for any other date.
    pub(super) fn day(&self, date: NaiveDate) -> Arc<Day> {
        if date == (self.clock)() {
            self.today(date)
        } else {
            Arc::new(self.load_day(date))
        }
    }

    /// The whole notebook tree, rescanned only after a change on disk.
    pub(super) fn notes(&self) -> Arc<Vec<Node>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let changed = self.dirty.notes.swap(false, Ordering::SeqCst) || self.watcher.is_none();
        match &cache.notes {
            Some((nodes, scanned)) if !changed && scanned.elapsed() < MAX_CACHE_AGE => {
                nodes.clone()
            }
            _ => {
                let nodes = Arc::new(scan_dir(&self.root, true));
                cache.notes = Some((nodes.clone(), Instant::now()));
                nodes
            }
        }
    }

    /// Today's folder, reloaded after a change on disk or when the date
    /// rolls over. `date` is today, read from the clock once by the caller.
    pub(super) fn today(&self, date: NaiveDate) -> Arc<Day> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let changed = self.dirty.daily.swap(false, Ordering::SeqCst) || self.watcher.is_none();
        match &cache.today {
            Some((day, loaded))
                if !changed
                    && day.date == date
                    && loaded.elapsed() < MAX_CACHE_AGE
                    && self.is_current(day) =>
            {
                day.clone()
            }
            _ => {
                let day = Arc::new(self.load_day(date));
                cache.today = Some((day.clone(), Instant::now()));
                day
            }
        }
    }

    /// Whether a cached day still matches the disk. This does not depend on
    /// the watcher, so a lost event cannot leave an old Summary in memory,
    /// or keep showing a day whose folder has since been hidden.
    pub(super) fn is_current(&self, day: &Day) -> bool {
        match (&day.folder, self.day_dir(day.date)) {
            (None, Err(_)) => true,
            (Some(folder), Ok(dir)) => {
                dir == folder.dir
                    && stamp(&dir) == folder.stamp
                    && folder.summary.as_ref().is_none_or(|summary| {
                        // A Summary that could not be read is tried again.
                        summary.text.is_ok() && stamp(&dir.join(&summary.name)) == summary.stamp
                    })
            }
            _ => false,
        }
    }
}
