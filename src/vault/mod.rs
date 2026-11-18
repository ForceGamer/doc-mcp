//! The notebook itself, and the three read-only operations the MCP tools
//! expose: `list_notes`, `read_note` and `read_all_notes`.
//!
//! The rest of the module is split by concern:
//!
//! - [`paths`]: turning a path from the caller into a place in the notebook,
//!   and writing places back as paths the caller can use.
//! - [`daily`]: the per-day folders, which are reached by date.
//! - [`visibility`]: which files and folders the tools may never see.
//! - [`entries`]: scanning directories into a tree of entries.
//! - [`cache`]: keeping the notebook tree and today's folder in memory.
//! - [`watch`]: the file watcher that tells the cache when to refresh.
//! - [`render`]: the text the tools send back.

mod cache;
mod daily;
mod entries;
mod paths;
mod render;
mod visibility;
mod watch;

#[cfg(test)]
mod tests;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::NaiveDate;
use notify::RecommendedWatcher;

use cache::Cache;
use entries::{Kind, Node, is_markdown, read_text};
use paths::{display_or_root, display_path};
use render::listing;
use visibility::still_listed;
use watch::Dirty;

/// Name of the directory that holds the per-day folders. It is never exposed
/// as a path; a day is reached with a date instead.
const DAILY_DIR: &str = "Daily";
/// The AI-written summary of a day, kept in each day's folder.
const SUMMARY_FILE: &str = "Summary.md";
/// Stands for the current date wherever a date is accepted.
const TODAY: &str = "today";

pub type Result<T> = std::result::Result<T, String>;

/// Supplies "today". Injected so tests can pin the date.
pub type Clock = Box<dyn Fn() -> NaiveDate + Send + Sync>;

pub struct Vault {
    /// Canonical notebook root.
    root: PathBuf,
    clock: Clock,
    cache: Mutex<Cache>,
    dirty: Arc<Dirty>,
    /// Keeps the watcher alive. Without one the cache is rebuilt on every call.
    watcher: Option<RecommendedWatcher>,
}

impl Vault {
    /// Open the notebook at `root`. With `watch`, a file watcher keeps the
    /// cache current; if it cannot be started the cache is rebuilt per call.
    pub fn open(root: &Path, clock: Clock, watch: bool) -> Result<Self> {
        let root = fs::canonicalize(root)
            .map_err(|e| format!("cannot open notes directory {}: {e}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a directory", root.display()));
        }
        let dirty = Arc::new(Dirty::default());
        let watcher = if watch {
            match watch::start_watcher(&root, dirty.clone()) {
                Ok(watcher) => Some(watcher),
                Err(e) => {
                    eprintln!("doc-mcp: file watching unavailable ({e}); rescanning on every call");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            root,
            clock,
            cache: Mutex::new(Cache::default()),
            dirty,
            watcher,
        })
    }

    /// List a directory, or a day's folder when `dir` is a date.
    pub fn list_notes(&self, dir: Option<&str>, tree: bool) -> Result<String> {
        let target = self.resolve(dir.unwrap_or(""))?;
        if !target.is_dir {
            return Err(format!(
                "{} is a file, not a directory. Use read_note to read it.",
                target.display()
            ));
        }
        let (nodes, day) = self.contents(&target)?;
        let mut out = listing(&target, &nodes, tree);

        if target.rel.is_empty() {
            out.push('\n');
            match day {
                // A day is listed together with its summary.
                Some(day) => match day.summary().map(|summary| &summary.text) {
                    Some(Ok(text)) => {
                        out.push_str("Summary of the day (Summary.md):\n");
                        out.push_str(text.trim_end());
                        out.push('\n');
                    }
                    Some(Err(e)) => {
                        out.push_str(&format!("Summary.md exists but could not be read: {e}\n"))
                    }
                    None => out.push_str("This day has no Summary.md.\n"),
                },
                // The notebook root is listed together with today's notes.
                None => out.push_str(&self.today_section(tree)),
            }
        }
        Ok(out)
    }

    /// Read one markdown note.
    pub fn read_note(&self, path: &str) -> Result<String> {
        let target = self.resolve(path)?;
        if target.is_dir {
            return Err(format!(
                "{} is a directory. Use list_notes or read_all_notes for directories.",
                display_or_root(&target)
            ));
        }
        if !is_markdown(target.rel.last().map(String::as_str).unwrap_or("")) {
            return Err(format!(
                "{} is not a markdown note. Only md files can be read.",
                target.display()
            ));
        }
        // Today's Summary is held in memory.
        if let Some(date) = target.date
            && date == (self.clock)()
            && let [name] = target.rel.as_slice()
            && let Some(summary) = self.day(date).summary()
            && summary.name == *name
            && let Ok(text) = &summary.text
        {
            return Ok(text.clone());
        }
        read_text(&target.abs).map_err(|e| format!("cannot read {}: {e}", target.display()))
    }

    /// List a directory and read every markdown note directly inside it,
    /// except Summary.md. The listing comes first, then one item per note,
    /// so that the text of one note cannot pass for the start of another.
    pub fn read_all_notes(&self, path: &str) -> Result<Vec<String>> {
        let target = self.resolve(path)?;
        if !target.is_dir {
            return Err(format!(
                "{} is a file, not a directory. Use read_note to read it.",
                target.display()
            ));
        }
        let (nodes, _) = self.contents(&target)?;
        let mut out = listing(&target, &nodes, false);

        let notes: Vec<&Node> = nodes
            .iter()
            .filter(|n| matches!(&n.kind, Kind::File(ext) if ext == "md"))
            .filter(|n| !n.name.eq_ignore_ascii_case(SUMMARY_FILE))
            // The listing may be a moment old.
            .filter(|n| still_listed(&target.abs, &n.name))
            .collect();
        if notes.is_empty() {
            out.push_str("\nThere are no md notes to read in this directory.\n");
        }
        let mut items = vec![out];
        for note in notes {
            let mut rel = target.rel.clone();
            rel.push(note.name.clone());
            let shown = display_path(target.day_name.as_deref(), &rel);
            // The text follows its heading exactly as it is on disk.
            items.push(match read_text(&target.abs.join(&note.name)) {
                Ok(text) => format!("===== {shown} =====\n{text}"),
                Err(e) => format!("===== {shown} =====\n(cannot read this note: {e})\n"),
            });
        }
        Ok(items)
    }
}
