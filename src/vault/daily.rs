//! Daily notes. Each day has its own folder, `Daily/YYYY/MM/DD-DAY`, which
//! the tools reach by date instead of by path.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{Datelike, NaiveDate, Weekday};

use super::{
    DAILY_DIR, Result, SUMMARY_FILE, Vault,
    entries::{Kind, Node, read_text, scan_dir},
    visibility::{is_ignored, still_listed},
};

/// A day's folder: its files and the whole Summary text.
#[derive(Debug)]
pub(super) struct Day {
    pub(super) date: NaiveDate,
    /// `None` when the folder for that date does not exist.
    pub(super) folder: Option<DayFolder>,
}

#[derive(Debug)]
pub(super) struct DayFolder {
    pub(super) dir: PathBuf,
    /// The folder as it was on disk when it was scanned.
    pub(super) stamp: Stamp,
    pub(super) nodes: Vec<Node>,
    pub(super) summary: Option<Summary>,
}

#[derive(Debug)]
pub(super) struct Summary {
    /// The file's name as it is on disk.
    pub(super) name: String,
    /// The whole text, or why the file could not be read.
    pub(super) text: std::result::Result<String, String>,
    /// The file as it was on disk when it was read.
    pub(super) stamp: Stamp,
}

/// Modification time and size, to notice that something changed on disk.
/// The size of a directory is its number of entries, because two changes
/// close together can leave the modification time the same.
pub(super) type Stamp = Option<(Option<SystemTime>, u64)>;

pub(super) fn stamp(path: &Path) -> Stamp {
    let meta = fs::metadata(path).ok()?;
    let size = if meta.is_dir() {
        fs::read_dir(path).ok()?.count() as u64
    } else {
        meta.len()
    };
    Some((meta.modified().ok(), size))
}

impl Day {
    pub(super) fn nodes(&self) -> Option<&[Node]> {
        self.folder.as_ref().map(|folder| folder.nodes.as_slice())
    }

    pub(super) fn summary(&self) -> Option<&Summary> {
        self.folder.as_ref()?.summary.as_ref()
    }
}

impl Vault {
    /// `<root>/Daily/YYYY/MM/DD-DAY` for a date. Every folder on the way has
    /// to be a real, visible directory: a link or a hidden folder there
    /// could otherwise lead a date to a place the tools must not see.
    pub(super) fn day_dir(&self, date: NaiveDate) -> Result<PathBuf> {
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

    /// Read a day's folder from disk.
    pub(super) fn load_day(&self, date: NaiveDate) -> Day {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_folder_names_follow_the_schema() {
        let date = |d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        assert_eq!(day_folder_name(date(3)), "03-TUE");
        assert_eq!(day_folder_name(date(1)), "01-SUN");
    }
}
