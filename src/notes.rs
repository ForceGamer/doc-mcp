//! The notebook itself: path rules, directory scanning, the cache, and the
//! three operations the MCP tools expose.
//!
//! Everything here is read-only.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use chrono::{Datelike, NaiveDate, Weekday};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Name of the directory that holds the per-day folders. It is never exposed
/// as a path; a day is reached with a date instead.
const DAILY_DIR: &str = "Daily";
/// The AI-written summary of a day, kept in each day's folder.
const SUMMARY_FILE: &str = "Summary.md";
/// Stands for the current date wherever a date is accepted.
const TODAY: &str = "today";

const DAILY_BY_DATE_ONLY: &str = "The Daily directory cannot be accessed by path. Use a date \
     instead: `YYYY-MM-DD` or `today` for the day's folder, and `YYYY-MM-DD/<file>` for a note in it.";

pub type Result<T> = std::result::Result<T, String>;

/// Supplies "today". Injected so tests can pin the date.
pub type Clock = Box<dyn Fn() -> NaiveDate + Send + Sync>;

/// One entry in a scanned directory.
#[derive(Debug, Clone)]
struct Node {
    name: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Dir(Vec<Node>),
    /// Lower-cased extension, or empty when the file has none.
    File(String),
}

/// The watcher is the normal way the cache is refreshed. Watchers can lose
/// events, so anything cached is also read again once it is this old.
const MAX_CACHE_AGE: Duration = Duration::from_secs(60);

/// Modification time and size, to notice that something changed on disk.
/// The size of a directory is its number of entries, because two changes
/// close together can leave the modification time the same.
type Stamp = Option<(Option<SystemTime>, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).ok()?;
    let size = if meta.is_dir() {
        fs::read_dir(path).ok()?.count() as u64
    } else {
        meta.len()
    };
    Some((meta.modified().ok(), size))
}

/// A day's folder: its files and the whole Summary text.
#[derive(Debug)]
struct Day {
    date: NaiveDate,
    /// `None` when the folder for that date does not exist.
    folder: Option<DayFolder>,
}

#[derive(Debug)]
struct DayFolder {
    dir: PathBuf,
    /// The folder as it was on disk when it was scanned.
    stamp: Stamp,
    nodes: Vec<Node>,
    summary: Option<Summary>,
}

#[derive(Debug)]
struct Summary {
    /// The file's name as it is on disk.
    name: String,
    /// The whole text, or why the file could not be read.
    text: std::result::Result<String, String>,
    /// The file as it was on disk when it was read.
    stamp: Stamp,
}

impl Day {
    fn nodes(&self) -> Option<&[Node]> {
        self.folder.as_ref().map(|folder| folder.nodes.as_slice())
    }

    fn summary(&self) -> Option<&Summary> {
        self.folder.as_ref()?.summary.as_ref()
    }
}

#[derive(Default)]
struct Cache {
    /// Every file and folder of the notebook (without Daily), and when it
    /// was scanned.
    notes: Option<(Arc<Vec<Node>>, Instant)>,
    /// Today's daily folder, and when it was loaded.
    today: Option<(Arc<Day>, Instant)>,
}

/// Set by the file watcher when something on disk changed.
#[derive(Default)]
struct Dirty {
    notes: AtomicBool,
    daily: AtomicBool,
}

impl Dirty {
    fn mark_all(&self) {
        self.notes.store(true, Ordering::SeqCst);
        self.daily.store(true, Ordering::SeqCst);
    }
}

pub struct Vault {
    /// Canonical notebook root.
    root: PathBuf,
    clock: Clock,
    cache: Mutex<Cache>,
    dirty: Arc<Dirty>,
    /// Keeps the watcher alive. Without one the cache is rebuilt on every call.
    watcher: Option<RecommendedWatcher>,
}

/// Where a path given by the caller points.
struct Resolved {
    /// `Some` when the path was addressed by date.
    date: Option<NaiveDate>,
    /// For a date, the name the day goes by in output.
    day_name: Option<String>,
    abs: PathBuf,
    /// Components below the notebook root, or below the day's folder.
    rel: Vec<String>,
    is_dir: bool,
}

impl Resolved {
    /// The path as the caller should write it, e.g. `Projects/Plan.md` or
    /// `2026-11-03/Meeting.md`.
    fn display(&self) -> String {
        display_path(self.day_name.as_deref(), &self.rel)
    }
}

fn display_path(day_name: Option<&str>, rel: &[String]) -> String {
    let parts: Vec<&str> = day_name
        .into_iter()
        .chain(rel.iter().map(String::as_str))
        .collect();
    parts.join("/")
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
            match start_watcher(&root, dirty.clone()) {
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

    // ---- tools -----------------------------------------------------------

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

    // ---- rendering -------------------------------------------------------

    /// Today's daily notes, appended to listings of the notebook root.
    fn today_section(&self, tree: bool) -> String {
        let today = self.today((self.clock)());
        let date = today.date.format("%Y-%m-%d").to_string();
        match (today.nodes(), self.day_name(today.date)) {
            (None, _) => format!("Today is {date}. Its daily folder does not exist yet.\n"),
            (Some(_), None) => format!(
                "Today's daily notes ({date}) cannot be opened: entries in the notebook root \
                 are named both `{TODAY}` and `{date}`. One of them has to be renamed.\n"
            ),
            (Some(nodes), Some(name)) => {
                let mut out = format!(
                    "Today's daily notes ({date}). Daily notes are addressed by date, \
                     for example `{name}` or `{name}/<file>`:\n"
                );
                if nodes.is_empty() {
                    out.push_str("(empty)\n");
                } else {
                    render(nodes, &name, tree, &mut out);
                }
                out
            }
        }
    }

    /// The name a day goes by in output: its date or, when an entry in the
    /// notebook root has taken that name, `today` if that is the day. `None`
    /// when no name is left that would open it.
    fn day_name(&self, date: NaiveDate) -> Option<String> {
        let written = date.format("%Y-%m-%d").to_string();
        if !self.shown_in_root(&written) {
            Some(written)
        } else if date == (self.clock)() && !self.shown_in_root(TODAY) {
            Some(TODAY.to_string())
        } else {
            None
        }
    }

    // ---- cache -----------------------------------------------------------

    /// The whole notebook tree, rescanned only after a change on disk.
    fn notes(&self) -> Arc<Vec<Node>> {
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
    fn today(&self, date: NaiveDate) -> Arc<Day> {
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
    fn is_current(&self, day: &Day) -> bool {
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

    /// A day's folder: from the cache for today, from disk for any other date.
    fn day(&self, date: NaiveDate) -> Arc<Day> {
        if date == (self.clock)() {
            self.today(date)
        } else {
            Arc::new(self.load_day(date))
        }
    }

    fn load_day(&self, date: NaiveDate) -> Day {
        let Ok(dir) = self.day_dir(date) else {
            return Day { date, folder: None };
        };
        // Each stamp is taken before the read it describes, so a change
        // during the read shows up as a mismatch on the next call.
        let dir_stamp = stamp(&dir);
        let nodes = scan_dir(&dir, false);
        let is_summary = |n: &&Node, exact: bool| {
            matches!(n.kind, Kind::File(_))
                && if exact {
                    n.name == SUMMARY_FILE
                } else {
                    n.name.eq_ignore_ascii_case(SUMMARY_FILE)
                }
        };
        let summary = nodes
            .iter()
            .find(|n| is_summary(n, true))
            .or_else(|| nodes.iter().find(|n| is_summary(n, false)))
            .map(|file| {
                let path = dir.join(&file.name);
                let stamp = stamp(&path);
                let text = if still_listed(&dir, &file.name) {
                    read_text(&path).map_err(|e| e.to_string())
                } else {
                    Err("it is no longer a plain file".to_string())
                };
                Summary {
                    name: file.name.clone(),
                    text,
                    stamp,
                }
            });
        Day {
            date,
            folder: Some(DayFolder {
                dir,
                stamp: dir_stamp,
                nodes,
                summary,
            }),
        }
    }

    /// The entries of a resolved directory and, for a date, the day they
    /// belong to. A tool call gets both from one look at the cache or disk.
    fn contents(&self, target: &Resolved) -> Result<(Vec<Node>, Option<Arc<Day>>)> {
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

    // ---- paths -----------------------------------------------------------

    /// `<root>/Daily/YYYY/MM/DD-DAY` for a date. Every folder on the way has
    /// to be a real, visible directory: a link or a hidden folder there
    /// could otherwise lead a date to a place the tools must not see.
    fn day_dir(&self, date: NaiveDate) -> Result<PathBuf> {
        let shown = date.format("%Y-%m-%d");
        let missing = || format!("There is no daily folder for {shown}.");

        let mut dir = self.root.clone();
        let steps = [
            DAILY_DIR.to_string(),
            format!("{:04}", date.year()),
            format!("{:02}", date.month()),
            day_folder_name(date),
        ];
        for wanted in &steps {
            // The folder with exactly this name, or else one that differs
            // only in letter case (`daily`, `03-Tue`).
            let name = if fs::symlink_metadata(dir.join(wanted)).is_ok() {
                wanted.clone()
            } else {
                fs::read_dir(&dir)
                    .map_err(|_| missing())?
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .find(|name| name.eq_ignore_ascii_case(wanted))
                    .ok_or_else(missing)?
            };
            dir.push(&name);
            let meta = fs::symlink_metadata(&dir).map_err(|_| missing())?;
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "The daily folder for {shown} is behind a link, and links are not followed."
                ));
            }
            if !meta.is_dir() || is_ignored(&name, &meta) {
                return Err(missing());
            }
        }

        let dir = fs::canonicalize(&dir).map_err(|_| missing())?;
        let depth = dir
            .strip_prefix(&self.root)
            .map(|rel| rel.components().count());
        if depth != Ok(4) {
            return Err(missing());
        }
        Ok(dir)
    }

    /// Turn a caller-supplied path into a location inside the notebook,
    /// applying every access rule. Anything the tools must not see is
    /// reported the same way as a path that does not exist.
    fn resolve(&self, input: &str) -> Result<Resolved> {
        let not_found = || format!("Not found: {}", input.trim());
        let parts = split_path(input).ok_or_else(not_found)?;

        let first = parts.first().map(String::as_str);
        let mut date = first.and_then(|p| parse_date(p, &self.clock));
        // An entry that listings show in the notebook root can have a name
        // that also reads as a date. It is opened as that entry, unless the
        // day exists too: then the name is refused, never silently guessed.
        if let (Some(day), Some(name)) = (date, first)
            && self.shown_in_root(name)
        {
            if self.day_dir(day).is_ok() {
                return Err(self.clash(name, day));
            }
            date = None;
        }
        let (base, rest) = match (date, first) {
            (Some(date), _) => (self.day_dir(date)?, &parts[1..]),
            (None, Some(name)) if name.eq_ignore_ascii_case(DAILY_DIR) => {
                return Err(DAILY_BY_DATE_ONLY.to_string());
            }
            (None, Some(name)) if date_shaped(name) && !self.shown_in_root(name) => {
                return Err(format!("`{name}` is not a valid date."));
            }
            (None, _) => (self.root.clone(), &parts[..]),
        };

        // Links are not followed, here or in listings.
        let mut lexical = base.clone();
        for part in rest {
            lexical.push(part);
            let meta = fs::symlink_metadata(&lexical).map_err(|_| not_found())?;
            if meta.file_type().is_symlink() {
                return Err(not_found());
            }
        }
        // Canonicalising settles letter case and Windows short names, so the
        // rules below judge the entry's real name.
        let abs = fs::canonicalize(&lexical).map_err(|_| not_found())?;
        let rel: Vec<String> = abs
            .strip_prefix(&base)
            .map_err(|_| not_found())?
            .components()
            .map(|c| match c {
                Component::Normal(name) => name.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Option<_>>()
            .ok_or_else(not_found)?;

        let mut is_dir = true;
        let mut current = base;
        for (depth, name) in rel.iter().enumerate() {
            current.push(name);
            let meta = fs::metadata(&current).map_err(|_| not_found())?;
            is_dir = meta.is_dir();
            if date.is_none() && depth == 0 && name.eq_ignore_ascii_case(DAILY_DIR) {
                return Err(DAILY_BY_DATE_ONLY.to_string());
            }
            // Pipes, sockets and devices are not notes.
            if is_ignored(name, &meta) || !(meta.is_dir() || meta.is_file()) {
                return Err(not_found());
            }
        }
        Ok(Resolved {
            date,
            // Resolving by date succeeded, so the day has a name that works;
            // the date itself is only a fallback.
            day_name: date.map(|day| {
                self.day_name(day)
                    .unwrap_or_else(|| day.format("%Y-%m-%d").to_string())
            }),
            abs,
            rel,
            is_dir,
        })
    }

    /// Whether listings show an entry of this name in the notebook root.
    fn shown_in_root(&self, name: &str) -> bool {
        !name.eq_ignore_ascii_case(DAILY_DIR)
            && fs::symlink_metadata(self.root.join(name))
                .is_ok_and(|meta| (meta.is_dir() || meta.is_file()) && !is_ignored(name, &meta))
    }

    /// The error for a name that is both an entry in the notebook root and
    /// a day that exists.
    fn clash(&self, name: &str, day: NaiveDate) -> String {
        let date = day.format("%Y-%m-%d").to_string();
        // The day may still have a name nothing else has taken.
        let other = if name.eq_ignore_ascii_case(TODAY) {
            Some(date.as_str())
        } else if day == (self.clock)() {
            Some(TODAY)
        } else {
            None
        };
        let way_in = match other {
            Some(other) if !self.shown_in_root(other) => {
                format!(" The daily folder can be opened as `{other}`.")
            }
            Some(other) => format!(
                " The daily folder's other name, `{other}`, is taken by an entry in the \
                 notebook root as well."
            ),
            None => String::new(),
        };
        format!(
            "`{name}` is the name of an entry in the notebook root and also of the daily \
             folder for {date}, so it is not opened as either.{way_in} The entry in the \
             notebook root cannot be opened until it is renamed."
        )
    }
}

/// Whether a file that a scan listed is still that plain, visible file, and
/// not a link or another file that has taken its place since.
fn still_listed(dir: &Path, name: &str) -> bool {
    let path = dir.join(name);
    fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file() && !is_ignored(name, &meta))
        && fs::canonicalize(&path).is_ok_and(|real| real.file_name().is_some_and(|n| n == name))
}

/// The heading and entries of a directory.
fn listing(target: &Resolved, nodes: &[Node], tree: bool) -> String {
    let what = if tree { "Everything in" } else { "Contents of" };
    let place = match (target.date, target.rel.is_empty()) {
        (None, true) => "the notebook".to_string(),
        (Some(date), true) => format!("the daily notes for {}", date.format("%Y-%m-%d")),
        _ => target.display(),
    };
    let mut out = format!("{what} {place}:\n");
    if nodes.is_empty() {
        out.push_str("(empty)\n");
    } else {
        render(nodes, &target.display(), tree, &mut out);
    }
    out
}

fn display_or_root(target: &Resolved) -> String {
    let shown = target.display();
    if shown.is_empty() {
        "the notebook root".to_string()
    } else {
        shown
    }
}

/// Split a caller path into components. Accepts `/` and `\`, with or without
/// a leading slash. Returns `None` for anything that could leave the notebook.
fn split_path(input: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for part in input.trim().split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            // No drive letters or NTFS alternate data streams.
            _ if part.contains(':') => return None,
            _ => parts.push(part.to_string()),
        }
    }
    Some(parts)
}

/// `today` or a strict `YYYY-MM-DD`.
fn parse_date(text: &str, clock: &Clock) -> Option<NaiveDate> {
    if text.eq_ignore_ascii_case(TODAY) {
        return Some(clock());
    }
    if !date_shaped(text) {
        return None;
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// Written like `YYYY-MM-DD`, whether or not that day exists in a calendar.
fn date_shaped(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
}

/// `03-TUE` for 2026-11-03.
fn day_folder_name(date: NaiveDate) -> String {
    let day = match date.weekday() {
        Weekday::Mon => "MON",
        Weekday::Tue => "TUE",
        Weekday::Wed => "WED",
        Weekday::Thu => "THU",
        Weekday::Fri => "FRI",
        Weekday::Sat => "SAT",
        Weekday::Sun => "SUN",
    };
    format!("{:02}-{day}", date.day())
}

fn is_markdown(name: &str) -> bool {
    file_type(name) == "md"
}

fn file_type(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// Entries no tool may see: hidden directories, and directories and files
/// whose name ends in `-s`.
fn is_ignored(name: &str, meta: &fs::Metadata) -> bool {
    ignored_by_name(name, meta.is_dir()) || (meta.is_dir() && has_hidden_attribute(meta))
}

fn ignored_by_name(name: &str, is_dir: bool) -> bool {
    let name = name.to_ascii_lowercase();
    // `-s` ends the name, or the part before any extension: `secret-s.md`
    // and `secret-s.backup.md` end in `-s` as far as the user is concerned.
    name.ends_with("-s") || name.contains("-s.") || (is_dir && name.starts_with('.'))
}

#[cfg(windows)]
fn has_hidden_attribute(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn has_hidden_attribute(_meta: &fs::Metadata) -> bool {
    false
}

/// Scan a directory recursively, leaving out everything the tools must not
/// see. Symlinks are not followed. Directories come first, then files.
fn scan_dir(dir: &Path, is_root: bool) -> Vec<Node> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if is_ignored(&name, &meta) {
            continue;
        }
        if meta.is_dir() {
            if is_root && name.eq_ignore_ascii_case(DAILY_DIR) {
                continue;
            }
            let children = scan_dir(&entry.path(), false);
            nodes.push(Node {
                name,
                kind: Kind::Dir(children),
            });
        } else if meta.is_file() {
            let kind = Kind::File(file_type(&name));
            nodes.push(Node { name, kind });
        }
    }
    nodes.sort_by_cached_key(|n| (matches!(n.kind, Kind::File(_)), n.name.to_lowercase()));
    nodes
}

/// The children of the directory at `rel` below `nodes`.
fn find<'a>(nodes: &'a [Node], rel: &[String]) -> Option<&'a [Node]> {
    let Some((first, rest)) = rel.split_first() else {
        return Some(nodes);
    };
    nodes.iter().find_map(|n| match &n.kind {
        Kind::Dir(children) if n.name == *first => find(children, rest),
        _ => None,
    })
}

fn describe(node: &Node, path: &str) -> String {
    match &node.kind {
        Kind::Dir(_) => format!("{path}/"),
        Kind::File(ext) if ext.is_empty() => format!("{path} (no file type)"),
        Kind::File(ext) => format!("{path} ({ext})"),
    }
}

/// One line per entry, each with its full path so it can be passed straight
/// back to a tool. `deep` also lists everything inside the directories.
fn render(nodes: &[Node], prefix: &str, deep: bool, out: &mut String) {
    for node in nodes {
        let path = if prefix.is_empty() {
            node.name.clone()
        } else {
            format!("{prefix}/{}", node.name)
        };
        out.push_str(&describe(node, &path));
        out.push('\n');
        if deep && let Kind::Dir(children) = &node.kind {
            render(children, &path, deep, out);
        }
    }
}

fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// Watch the notebook and flag the affected part of the cache on any change.
fn start_watcher(root: &Path, dirty: Arc<Dirty>) -> notify::Result<RecommendedWatcher> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory link: a junction on Windows, a symlink elsewhere.
    fn link_dir(target: &Path, link: &Path) {
        #[cfg(windows)]
        {
            // mklink reads a forward slash as the start of a switch.
            let native = |path: &Path| path.components().collect::<PathBuf>();
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .args([native(link), native(target)])
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(made.success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    const TODAY: (i32, u32, u32) = (2026, 11, 17);

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn notebook() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Inbox.md", "inbox text");
        write(root, "secret-s.md", "hidden file");
        write(root, "Projects/Plan.md", "the plan");
        write(root, "Projects/Summary.md", "project summary");
        write(root, "Projects/budget.xlsx", "x");
        write(root, "Projects/Alpha/Spec.md", "the spec");
        write(root, "Projects/Private-s/Diary.md", "private");
        write(root, ".obsidian/app.json", "{}");
        write(root, "Daily/2026/11/03-TUE/Summary.md", "A quiet Tuesday.");
        write(root, "Daily/2026/11/03-TUE/Meeting.md", "met people");
        write(root, "Daily/2026/11/03-TUE/report.docx", "x");
        write(root, "Daily/2026/11/17-TUE/Summary.md", "Today so far.");
        write(root, "Daily/2026/11/17-TUE/Standup.md", "standup notes");
        dir
    }

    fn vault(dir: &tempfile::TempDir, watch: bool) -> Vault {
        let clock: Clock = Box::new(|| date(TODAY.0, TODAY.1, TODAY.2));
        Vault::open(dir.path(), clock, watch).unwrap()
    }

    #[test]
    fn day_folder_names_follow_the_schema() {
        assert_eq!(day_folder_name(date(2026, 11, 3)), "03-TUE");
        assert_eq!(day_folder_name(date(2026, 11, 1)), "01-SUN");
    }

    #[test]
    fn root_listing_hides_daily_hidden_and_dash_s() {
        let dir = notebook();
        let out = vault(&dir, false).list_notes(None, false).unwrap();
        assert!(out.contains("Projects/\n"), "{out}");
        assert!(out.contains("Inbox.md (md)"), "{out}");
        assert!(!out.contains("Daily/"), "{out}");
        assert!(!out.contains(".obsidian"), "{out}");
        assert!(!out.contains("secret-s"), "{out}");
        // Today's notes are offered by date.
        assert!(out.contains("2026-11-17/Standup.md (md)"), "{out}");
    }

    #[test]
    fn tree_shows_the_whole_notebook() {
        let dir = notebook();
        let out = vault(&dir, false).list_notes(None, true).unwrap();
        assert!(
            out.contains("Projects/\nProjects/Alpha/\nProjects/Alpha/Spec.md (md)\n"),
            "{out}"
        );
        assert!(out.contains("\nProjects/budget.xlsx (xlsx)"), "{out}");
        assert!(!out.contains("Private-s"), "{out}");
        assert!(!out.contains("Diary"), "{out}");
    }

    #[test]
    fn directory_listing_gives_full_paths() {
        let dir = notebook();
        let v = vault(&dir, false);
        let out = v.list_notes(Some("/Projects/"), false).unwrap();
        assert!(out.contains("Projects/Alpha/\n"), "{out}");
        assert!(out.contains("Projects/Plan.md (md)"), "{out}");
        assert!(!out.contains("Spec.md"), "{out}");
        assert!(
            v.list_notes(Some("Projects\\Alpha"), false)
                .unwrap()
                .contains("Spec.md")
        );
        assert!(
            v.list_notes(Some("Inbox.md"), false)
                .unwrap_err()
                .contains("not a directory")
        );
    }

    #[test]
    fn daily_is_reached_by_date_only() {
        let dir = notebook();
        let v = vault(&dir, false);
        for path in [
            "Daily",
            "daily/2026",
            "/Daily/2026/11/03-TUE",
            "Daily/2026/11/03-TUE/Meeting.md",
        ] {
            assert_eq!(
                v.list_notes(Some(path), false).unwrap_err(),
                DAILY_BY_DATE_ONLY
            );
            assert_eq!(v.read_note(path).unwrap_err(), DAILY_BY_DATE_ONLY);
            assert_eq!(v.read_all_notes(path).unwrap_err(), DAILY_BY_DATE_ONLY);
        }
        let out = v.list_notes(Some("2026-11-03"), false).unwrap();
        assert!(out.contains("2026-11-03/Meeting.md (md)"), "{out}");
        assert!(out.contains("2026-11-03/report.docx (docx)"), "{out}");
        assert!(out.contains("A quiet Tuesday."), "{out}");
        assert_eq!(v.read_note("2026-11-03/Meeting.md").unwrap(), "met people");
        assert!(
            v.list_notes(Some("2026-11-04"), false)
                .unwrap_err()
                .contains("no daily folder")
        );
    }

    #[test]
    fn today_resolves_to_the_current_day() {
        let dir = notebook();
        let v = vault(&dir, false);
        let out = v.list_notes(Some("today"), false).unwrap();
        assert!(out.contains("2026-11-17/Standup.md (md)"), "{out}");
        assert!(out.contains("Today so far."), "{out}");
        assert_eq!(v.read_note("today/Summary.md").unwrap(), "Today so far.");
    }

    #[test]
    fn read_note_requires_a_visible_markdown_file() {
        let dir = notebook();
        let v = vault(&dir, false);
        assert_eq!(v.read_note("Projects/Plan.md").unwrap(), "the plan");
        assert!(
            v.read_note("Projects/budget.xlsx")
                .unwrap_err()
                .contains("Only md files")
        );
        assert!(
            v.read_note("Projects")
                .unwrap_err()
                .contains("is a directory")
        );
        for path in [
            "secret-s.md",
            "Projects/Private-s/Diary.md",
            ".obsidian/app.json",
            "../outside.md",
            "Projects/../../outside.md",
            "C:/Windows/win.ini",
            "Missing.md",
        ] {
            let err = v.read_note(path).unwrap_err();
            assert!(err.starts_with("Not found"), "{path}: {err}");
        }
    }

    #[test]
    fn read_all_notes_reads_markdown_but_not_the_summary() {
        let dir = notebook();
        let v = vault(&dir, false);
        let out = v.read_all_notes("Projects").unwrap().join("\n");
        assert!(
            out.contains("===== Projects/Plan.md =====\nthe plan"),
            "{out}"
        );
        assert!(!out.contains("project summary"), "{out}");
        assert!(!out.contains("the spec"), "{out}");
        assert!(!out.contains("private"), "{out}");
        assert!(out.contains("Projects/budget.xlsx (xlsx)"), "{out}");

        let day = v.read_all_notes("2026-11-03").unwrap().join("\n");
        assert!(
            day.contains("===== 2026-11-03/Meeting.md =====\nmet people"),
            "{day}"
        );
        assert!(!day.contains("A quiet Tuesday."), "{day}");
        assert!(
            v.read_all_notes("Inbox.md")
                .unwrap_err()
                .contains("not a directory")
        );
    }

    #[test]
    fn ignore_rules_by_name() {
        assert!(ignored_by_name(".git", true));
        assert!(ignored_by_name("Archive-s", true));
        assert!(ignored_by_name("notes-S", false));
        assert!(ignored_by_name("secret-s.md", false));
        assert!(ignored_by_name("secret-s.backup.md", false));
        assert!(ignored_by_name("secret-s.md.bak", false));
        assert!(ignored_by_name("Archive-s.old", true));
        assert!(!ignored_by_name("this-is.md", false));
        assert!(!ignored_by_name(".gitignore", false));
        assert!(!ignored_by_name("Notes", true));
        assert!(!ignored_by_name("glass-shards.md", false));
    }

    #[test]
    fn today_follows_the_clock() {
        let dir = notebook();
        let day = Arc::new(Mutex::new(date(2026, 11, 17)));
        let shared = day.clone();
        let v = Vault::open(dir.path(), Box::new(move || *shared.lock().unwrap()), true).unwrap();
        assert!(
            v.list_notes(None, false)
                .unwrap()
                .contains("2026-11-17/Standup.md")
        );
        *day.lock().unwrap() = date(2026, 11, 3);
        let out = v.list_notes(None, false).unwrap();
        assert!(out.contains("2026-11-03/Meeting.md"), "{out}");
        assert_eq!(v.read_note("today/Summary.md").unwrap(), "A quiet Tuesday.");
    }

    /// Poll until the watcher has delivered a change.
    fn eventually(mut check: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn cache_is_reused_until_the_watcher_reports_a_change() {
        let dir = notebook();
        let v = vault(&dir, true);
        assert!(v.watcher.is_some(), "file watching should work in tests");

        // Events from creating the notebook can still trickle in; once they
        // have, an unchanged notebook is served from memory.
        assert!(eventually(|| Arc::ptr_eq(&v.notes(), &v.notes())));
        let today = date(TODAY.0, TODAY.1, TODAY.2);
        assert!(eventually(|| Arc::ptr_eq(&v.today(today), &v.today(today))));
        assert_eq!(v.read_note("today/Summary.md").unwrap(), "Today so far.");

        write(dir.path(), "Projects/New.md", "new note");
        assert!(eventually(|| v
            .list_notes(None, true)
            .unwrap()
            .contains("New.md")));

        write(dir.path(), "Daily/2026/11/17-TUE/Summary.md", "Rewritten.");
        assert!(eventually(
            || v.read_note("today/Summary.md").unwrap() == "Rewritten."
        ));

        write(dir.path(), "Daily/2026/11/17-TUE/Late.md", "late");
        assert!(eventually(|| v
            .list_notes(Some("today"), false)
            .unwrap()
            .contains("Late.md")));
    }

    #[test]
    fn a_new_directory_is_listable_before_the_watcher_catches_up() {
        let dir = notebook();
        let v = vault(&dir, true);
        v.notes();
        write(dir.path(), "Fresh/Note.md", "fresh");
        assert!(
            v.list_notes(Some("Fresh"), false)
                .unwrap()
                .contains("Fresh/Note.md (md)")
        );
    }

    #[test]
    fn a_cached_day_is_checked_against_the_disk() {
        let dir = notebook();
        let v = vault(&dir, false);
        let day = v.load_day(date(TODAY.0, TODAY.1, TODAY.2));
        assert!(v.is_current(&day));

        // Even if the watcher never reports it, a rewritten Summary or a
        // new note is noticed.
        write(
            dir.path(),
            "Daily/2026/11/17-TUE/Summary.md",
            "Rewritten, and longer.",
        );
        assert!(!v.is_current(&day));
        let day = v.load_day(day.date);
        assert!(v.is_current(&day));
        write(dir.path(), "Daily/2026/11/17-TUE/Late.md", "late");
        assert!(!v.is_current(&day));

        // A day that had no folder is noticed once the folder appears.
        let none = v.load_day(date(2026, 11, 18));
        assert!(none.folder.is_none() && v.is_current(&none));
        write(dir.path(), "Daily/2026/11/18-WED/Note.md", "wednesday");
        assert!(!v.is_current(&none));
    }

    #[test]
    fn a_real_folder_wins_over_reading_its_name_as_a_date() {
        let dir = notebook();
        write(dir.path(), "2026-01-15/note.md", "january");
        write(dir.path(), "2026-11-03/real.md", "real");
        let v = vault(&dir, false);

        // No day has this date, so the name can only mean the folder.
        assert_eq!(v.read_note("2026-01-15/note.md").unwrap(), "january");
        let listed = v.list_notes(Some("2026-01-15"), false).unwrap();
        assert!(listed.contains("2026-01-15/note.md (md)"), "{listed}");
        // The date still reaches the day when no folder has taken its name.
        let today = v.list_notes(Some("2026-11-17"), false).unwrap();
        assert!(today.contains("2026-11-17/Standup.md"), "{today}");
        // A name that is both is refused, with the reason.
        for path in ["2026-11-03", "2026-11-03/real.md", "2026-11-03/Meeting.md"] {
            let err = v.read_all_notes(path).unwrap_err();
            assert!(err.contains("not opened as either"), "{path}: {err}");
        }
    }

    #[test]
    fn a_root_folder_named_today_is_never_opened_in_place_of_the_day() {
        let dir = notebook();
        write(dir.path(), "Today/Summary.md", "not the day");
        let v = vault(&dir, false);

        // The name is refused, and the error says how to reach the day.
        let err = v.read_note("Today/Summary.md").unwrap_err();
        assert!(err.contains("can be opened as `2026-11-17`"), "{err}");
        assert_eq!(
            v.read_note("2026-11-17/Summary.md").unwrap(),
            "Today so far."
        );

        // When the date is taken instead, listings offer the day as `today`.
        fs::rename(dir.path().join("Today"), dir.path().join("2026-11-17")).unwrap();
        let root = v.list_notes(None, false).unwrap();
        assert!(root.contains("today/Standup.md (md)"), "{root}");
        let err = v.read_note("2026-11-17/Standup.md").unwrap_err();
        assert!(err.contains("can be opened as `today`"), "{err}");
        assert_eq!(v.read_note("today/Standup.md").unwrap(), "standup notes");
        // Every output names the day in the way that opens it.
        let day = v.list_notes(Some("today"), false).unwrap();
        assert!(day.contains("today/Standup.md (md)"), "{day}");
        let all = v.read_all_notes("today").unwrap().join("\n");
        assert!(all.contains("===== today/Standup.md ====="), "{all}");

        // With both names taken, nothing is offered that would be refused.
        write(dir.path(), "Today/Summary.md", "not the day");
        let root = v.list_notes(None, false).unwrap();
        assert!(
            root.contains("cannot be opened") && !root.contains("Standup.md"),
            "{root}"
        );
        let err = v.read_note("today/Standup.md").unwrap_err();
        assert!(err.contains("is taken by an entry"), "{err}");
    }

    #[test]
    fn the_root_tree_includes_everything_in_todays_folder() {
        let dir = notebook();
        write(dir.path(), "Daily/2026/11/17-TUE/Sub/Nested.md", "nested");
        let v = vault(&dir, false);
        let flat = v.list_notes(None, false).unwrap();
        assert!(
            flat.contains("2026-11-17/Sub/\n") && !flat.contains("Nested.md"),
            "{flat}"
        );
        let tree = v.list_notes(None, true).unwrap();
        assert!(tree.contains("2026-11-17/Sub/Nested.md (md)"), "{tree}");
    }

    #[test]
    fn an_impossible_date_is_reported_as_one() {
        let dir = notebook();
        let v = vault(&dir, false);
        let err = v.list_notes(Some("2026-02-30"), false).unwrap_err();
        assert_eq!(err, "`2026-02-30` is not a valid date.");
    }

    #[test]
    fn read_all_notes_keeps_each_note_apart() {
        let dir = notebook();
        write(
            dir.path(),
            "Projects/Fake.md",
            "===== Projects/Other.md =====\nnot another note",
        );
        let v = vault(&dir, false);
        let items = v.read_all_notes("Projects").unwrap();
        // The listing, then one item for each of Fake.md and Plan.md.
        assert_eq!(items.len(), 3, "{items:?}");
        assert!(items[0].starts_with("Contents of Projects:"), "{items:?}");
        assert!(
            items[1].starts_with("===== Projects/Fake.md =====\n"),
            "{items:?}"
        );
        assert_eq!(items[2], "===== Projects/Plan.md =====\nthe plan");

        // The text is the file's, down to its trailing blank lines.
        write(dir.path(), "Projects/Plan.md", "the plan  \n\n");
        let items = v.read_all_notes("Projects").unwrap();
        assert_eq!(items[2], "===== Projects/Plan.md =====\nthe plan  \n\n");
    }

    #[cfg(windows)]
    #[test]
    fn an_unreadable_summary_is_reported_and_tried_again() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = notebook();
        let v = vault(&dir, true);
        let summary = dir.path().join("Daily/2026/11/17-TUE/Summary.md");

        // Another program holds the file without sharing it.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&summary)
            .unwrap();
        let out = v.list_notes(Some("today"), false).unwrap();
        assert!(
            out.contains("Summary.md exists but could not be read"),
            "{out}"
        );

        // Letting go of it changes nothing on disk, yet the next call reads it.
        drop(held);
        let out = v.list_notes(Some("today"), false).unwrap();
        assert!(out.contains("Today so far."), "{out}");
    }

    #[cfg(windows)]
    #[test]
    fn a_hidden_root_entry_does_not_take_a_date_away() {
        let dir = notebook();
        write(dir.path(), "2026-11-03/unseen.md", "unseen");
        let hidden = std::process::Command::new("attrib")
            .arg("+h")
            .arg(dir.path().join("2026-11-03"))
            .status()
            .unwrap();
        assert!(hidden.success());
        let v = vault(&dir, false);

        // No listing shows the hidden folder, so the name means the day.
        assert_eq!(v.read_note("2026-11-03/Meeting.md").unwrap(), "met people");
        let err = v.read_note("2026-11-03/unseen.md").unwrap_err();
        assert!(err.starts_with("Not found"), "{err}");
    }

    #[test]
    fn links_are_not_followed() {
        let dir = notebook();
        let root = dir.path();
        // A month of Daily that is really a link to an ignored folder.
        write(root, "Vault-s/03-SAT/secret.md", "secret");
        link_dir(&root.join("Vault-s"), &root.join("Daily/2026/10"));
        // A link in the notebook to a folder outside it, and to one inside.
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "out.md", "outside");
        link_dir(outside.path(), &root.join("Linked"));
        link_dir(&root.join("Projects"), &root.join("Alias"));
        let v = vault(&dir, false);

        let err = v.read_all_notes("2026-10-03").unwrap_err();
        assert!(err.contains("behind a link"), "{err}");
        assert!(v.read_note("2026-10-03/secret.md").is_err());
        for path in ["Linked", "Linked/out.md", "Alias/Plan.md"] {
            let err = v.read_note(path).unwrap_err();
            assert!(err.starts_with("Not found"), "{path}: {err}");
        }
        let tree = v.list_notes(None, true).unwrap();
        assert!(
            !tree.contains("Linked") && !tree.contains("Alias"),
            "{tree}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_note_replaced_by_a_link_is_not_read() {
        let dir = notebook();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "out.md", "outside");
        let v = vault(&dir, true);
        v.notes();
        // Whether or not the watcher has caught up, the link is not read.
        fs::remove_file(dir.path().join("Inbox.md")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("out.md"), dir.path().join("Inbox.md"))
            .unwrap();
        let out = v.read_all_notes("").unwrap().join("\n");
        assert!(!out.contains("outside"), "{out}");
    }

    #[cfg(windows)]
    #[test]
    fn hidden_folders_are_ignored_also_on_the_way_to_a_day() {
        let dir = notebook();
        let hide = |rel: &str| {
            let done = std::process::Command::new("attrib")
                .arg("+h")
                .arg(dir.path().join(rel))
                .status()
                .unwrap();
            assert!(done.success());
        };
        hide("Projects/Alpha");
        hide("Daily/2026/11/03-TUE");
        let v = vault(&dir, false);

        assert!(!v.list_notes(None, true).unwrap().contains("Alpha"));
        let err = v.read_note("Projects/Alpha/Spec.md").unwrap_err();
        assert!(err.starts_with("Not found"), "{err}");
        for path in ["2026-11-03", "2026-11-03/Meeting.md"] {
            let err = v.read_note(path).unwrap_err();
            assert!(err.contains("no daily folder"), "{path}: {err}");
        }
    }
}
