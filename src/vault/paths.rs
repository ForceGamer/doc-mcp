//! Turning a path from the caller into a place in the notebook. This is where
//! the access rules are enforced: nothing outside the notebook, no links, no
//! ignored entries, and the Daily directory only by date.
//!
//! It also covers the way back: how a place is written in output, so that
//! every path shown to the caller is one that opens it.

use std::{
    fs,
    path::{Component, PathBuf},
};

use chrono::NaiveDate;

use super::{Clock, DAILY_DIR, Result, TODAY, Vault, visibility::is_ignored};

pub(super) const DAILY_BY_DATE_ONLY: &str = "The Daily directory cannot be accessed by path. Use \
     a date instead: `YYYY-MM-DD` or `today` for the day's folder, and `YYYY-MM-DD/<file>` for a \
     note in it.";

/// Where a path given by the caller points.
pub(super) struct Resolved {
    /// `Some` when the path was addressed by date.
    pub(super) date: Option<NaiveDate>,
    /// For a date, the name the day goes by in output.
    pub(super) day_name: Option<String>,
    pub(super) abs: PathBuf,
    /// Components below the notebook root, or below the day's folder.
    pub(super) rel: Vec<String>,
    pub(super) is_dir: bool,
}

impl Resolved {
    /// The path as the caller should write it, e.g. `Projects/Plan.md` or
    /// `2026-11-03/Meeting.md`.
    pub(super) fn display(&self) -> String {
        display_path(self.day_name.as_deref(), &self.rel)
    }
}

/// A path as the caller writes it: below the notebook root, or below a day
/// when `day_name` is given.
pub(super) fn display_path(day_name: Option<&str>, rel: &[String]) -> String {
    let parts: Vec<&str> = day_name
        .into_iter()
        .chain(rel.iter().map(String::as_str))
        .collect();
    parts.join("/")
}

/// Like [`Resolved::display`], but names the notebook root instead of
/// showing an empty path.
pub(super) fn display_or_root(target: &Resolved) -> String {
    let shown = target.display();
    if shown.is_empty() {
        "the notebook root".to_string()
    } else {
        shown
    }
}

impl Vault {
    /// Turn a caller-supplied path into a location inside the notebook,
    /// applying every access rule. Anything the tools must not see is
    /// reported the same way as a path that does not exist.
    pub(super) fn resolve(&self, input: &str) -> Result<Resolved> {
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

    /// The name a day goes by in output: its date or, when an entry in the
    /// notebook root has taken that name, `today` if that is the day. `None`
    /// when no name is left that would open it.
    pub(super) fn day_name(&self, date: NaiveDate) -> Option<String> {
        let written = date.format("%Y-%m-%d").to_string();
        if !self.shown_in_root(&written) {
            Some(written)
        } else if date == (self.clock)() && !self.shown_in_root(TODAY) {
            Some(TODAY.to_string())
        } else {
            None
        }
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
