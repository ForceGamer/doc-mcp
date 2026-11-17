//! doc-mcp: a read-only MCP server (stdio) over a folder of markdown notes.

mod notes;

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

use notes::Vault;

const USAGE: &str = "Usage: doc-mcp <NOTES_DIR>

Serves the notes in NOTES_DIR to an MCP client over stdio.
NOTES_DIR can also be given in the NOTES_DIR environment variable.";

const INSTRUCTIONS: &str = "Read-only access to the user's notebook of markdown notes.

Paths are relative to the notebook root and use forward slashes, for example \
`Projects/Plan.md`. An empty path is the root.

Daily notes are different: each day has its own folder, and it is addressed by date \
instead of by path. Use `YYYY-MM-DD` (or `today`) wherever a directory is expected, and \
`YYYY-MM-DD/<file>` for a note of that day. Each day may have a Summary.md, a short \
summary of that day; list_notes shows it when given a date. The days themselves cannot be \
listed: ask for a day by its date, and expect an error if it has no folder.

Start with list_notes to see what exists. Only md files can be read.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListNotesArgs {
    /// Directory to list, relative to the notebook root (e.g. `Projects`), or
    /// a date (`YYYY-MM-DD` or `today`) to list that day's daily notes. Omit
    /// to list the notebook root.
    #[serde(default, alias = "path")]
    dir: Option<String>,
    /// Show everything beneath the directory instead of one level. With no
    /// `dir` this is the entire notebook tree.
    #[serde(default)]
    tree: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadNoteArgs {
    /// Path of an md file relative to the notebook root (e.g.
    /// `Projects/Plan.md`), or `YYYY-MM-DD/<file>` (or `today/<file>`) for a
    /// daily note.
    path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadAllNotesArgs {
    /// Directory relative to the notebook root (e.g. `Projects`), or a date
    /// (`YYYY-MM-DD` or `today`) for that day's daily notes. An empty string
    /// is the notebook root.
    path: String,
}

#[derive(Clone)]
struct NoteServer {
    vault: Arc<Vault>,
}

#[tool_router]
impl NoteServer {
    fn new(vault: Vault) -> Self {
        Self {
            vault: Arc::new(vault),
        }
    }

    #[tool(
        description = "List the folders and files in a directory of the notebook, each file \
            with its file type. With no arguments it lists the notebook root and today's daily \
            notes. Give a date as `dir` to list that day's daily notes together with the day's \
            summary. Set `tree` to see everything beneath the directory. Every entry is shown \
            with the full path to pass back to these tools.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_notes(&self, Parameters(args): Parameters<ListNotesArgs>) -> CallToolResult {
        self.run(move |vault| {
            vault
                .list_notes(args.dir.as_deref(), args.tree)
                .map(|text| vec![text])
        })
        .await
    }

    #[tool(
        description = "Read one note and return its text. The path must point to an md file, \
            written the way list_notes shows it: `Projects/Plan.md` for a note in the notebook, \
            `YYYY-MM-DD/<file>` or `today/<file>` for a daily note. A day's summary is read this \
            way too, as `YYYY-MM-DD/Summary.md`.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_note(&self, Parameters(args): Parameters<ReadNoteArgs>) -> CallToolResult {
        self.run(move |vault| vault.read_note(&args.path).map(|text| vec![text]))
            .await
    }

    #[tool(
        description = "List a directory and read every md note directly inside it in one call. \
            The path must be a directory or a date. The result is the listing followed by one \
            item per note, each starting with a line that gives the note's path. Notes in \
            subdirectories are not read, and neither is Summary.md; use read_note for that.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_all_notes(
        &self,
        Parameters(args): Parameters<ReadAllNotesArgs>,
    ) -> CallToolResult {
        self.run(move |vault| vault.read_all_notes(&args.path))
            .await
    }

    /// Run a notebook operation off the async threads, since it reads the
    /// disk, and turn its outcome into a tool result. Problems with a request
    /// (a missing note, a path that is not md) go back to the caller as a
    /// tool error it can read and act on.
    async fn run(
        &self,
        work: impl FnOnce(&Vault) -> notes::Result<Vec<String>> + Send + 'static,
    ) -> CallToolResult {
        let vault = self.vault.clone();
        let content = |texts: Vec<String>| texts.into_iter().map(ContentBlock::text).collect();
        match tokio::task::spawn_blocking(move || work(&vault)).await {
            Ok(Ok(texts)) => CallToolResult::success(content(texts)),
            Ok(Err(message)) => CallToolResult::error(content(vec![message])),
            Err(e) => CallToolResult::error(content(vec![format!("The tool failed: {e}")])),
        }
    }
}

#[tool_handler]
impl ServerHandler for NoteServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let arg = std::env::args_os().nth(1);
    if arg.as_ref().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(root) = arg
        .or_else(|| std::env::var_os("NOTES_DIR"))
        .map(PathBuf::from)
    else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let today = Box::new(|| chrono::Local::now().date_naive());
    let vault = match Vault::open(&root, today, true) {
        Ok(vault) => vault,
        Err(message) => {
            eprintln!("doc-mcp: {message}");
            return ExitCode::FAILURE;
        }
    };
    // stdout carries the protocol, so anything for a human goes to stderr.
    eprintln!("doc-mcp: serving {}", root.display());

    let service = match NoteServer::new(vault).serve(stdio()).await {
        Ok(service) => service,
        Err(e) => {
            eprintln!("doc-mcp: failed to start: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = service.waiting().await {
        eprintln!("doc-mcp: stopped with an error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
