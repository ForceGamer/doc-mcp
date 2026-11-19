//! Files and folders as the tools see them: scanned from disk into a tree,
//! with everything the tools must not see already left out.

use std::{fs, path::Path};

use super::{DAILY_DIR, visibility::is_ignored};

/// One entry in a scanned directory.
#[derive(Debug, Clone)]
pub(super) struct Node {
    pub(super) name: String,
    pub(super) kind: Kind,
}

#[derive(Debug, Clone)]
pub(super) enum Kind {
    Dir(Vec<Node>),
    /// Lower-cased extension, or empty when the file has none.
    File(String),
}

/// Scan a directory recursively, leaving out everything the tools must not
/// see. Symlinks are not followed. Directories come first, then files.
/// In the notebook root (`is_root`), the Daily directory is left out too.
pub(super) fn scan_dir(dir: &Path, is_root: bool) -> Vec<Node> {
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
pub(super) fn find<'a>(nodes: &'a [Node], rel: &[String]) -> Option<&'a [Node]> {
    let Some((first, rest)) = rel.split_first() else {
        return Some(nodes);
    };
    nodes.iter().find_map(|n| match &n.kind {
        Kind::Dir(children) if n.name == *first => find(children, rest),
        _ => None,
    })
}

pub(super) fn is_markdown(name: &str) -> bool {
    file_type(name) == "md"
}

/// The lower-cased extension of a file name, or empty when it has none.
fn file_type(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// A file's text. Bytes that are not valid UTF-8 are replaced, not refused.
pub(super) fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}
