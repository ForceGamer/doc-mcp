//! The rules for what no tool may see: hidden directories, and directories
//! and files whose name ends in `-s`.

use std::{fs, path::Path};

/// Whether an entry is hidden from the tools.
pub(super) fn is_ignored(name: &str, meta: &fs::Metadata) -> bool {
    ignored_by_name(name, meta.is_dir()) || (meta.is_dir() && has_hidden_attribute(meta))
}

/// The part of the rules that only needs the name.
pub(super) fn ignored_by_name(name: &str, is_dir: bool) -> bool {
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

/// Whether a file that a scan listed is still that plain, visible file, and
/// not a link or another file that has taken its place since.
pub(super) fn still_listed(dir: &Path, name: &str) -> bool {
    let path = dir.join(name);
    fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file() && !is_ignored(name, &meta))
        && fs::canonicalize(&path).is_ok_and(|real| real.file_name().is_some_and(|n| n == name))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
