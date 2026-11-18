//! Behaviour of the three operations, against a small notebook on disk.

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use chrono::NaiveDate;

use super::{Clock, Vault, paths::DAILY_BY_DATE_ONLY};

/// A directory link: a junction on Windows, a symlink elsewhere.
fn link_dir(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        // mklink reads a forward slash as the start of a switch.
        let native = |path: &Path| path.components().collect::<std::path::PathBuf>();
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
    std::os::unix::fs::symlink(outside.path().join("out.md"), dir.path().join("Inbox.md")).unwrap();
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
