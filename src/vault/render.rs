//! The text the tools send back. Every entry is shown with its full path, so
//! that it can be passed straight back to a tool.

use super::{
    TODAY, Vault,
    entries::{Kind, Node},
    paths::Resolved,
};

impl Vault {
    /// Today's daily notes, appended to listings of the notebook root.
    pub(super) fn today_section(&self, tree: bool) -> String {
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
}

/// The heading and entries of a directory.
pub(super) fn listing(target: &Resolved, nodes: &[Node], tree: bool) -> String {
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

/// One line per entry, each with its full path. `deep` also lists
/// everything inside the directories.
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

/// `Projects/` for a folder, `Projects/Plan.md (md)` for a file.
fn describe(node: &Node, path: &str) -> String {
    match &node.kind {
        Kind::Dir(_) => format!("{path}/"),
        Kind::File(ext) if ext.is_empty() => format!("{path} (no file type)"),
        Kind::File(ext) => format!("{path} ({ext})"),
    }
}
