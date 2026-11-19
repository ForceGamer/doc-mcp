//! doc-mcp: a read-only MCP server (stdio) over a folder of markdown notes.
//!
//! - [`server`]: the MCP tools and what the client is told about them.
//! - [`vault`]: the notebook, its access rules, and the work behind each tool.

mod server;
mod vault;

use std::{path::PathBuf, process::ExitCode};

use rmcp::{ServiceExt, transport::stdio};

use server::NoteServer;
use vault::Vault;

const USAGE: &str = "Usage: doc-mcp <NOTES_DIR>

Serves the notes in NOTES_DIR to an MCP client over stdio.
NOTES_DIR can also be given in the NOTES_DIR environment variable, or as
NOTES_DIR=<path> in a .env file in the working directory or one of its parents.
The argument wins over the environment, and the environment over .env.";

#[tokio::main]
async fn main() -> ExitCode {
    let arg = std::env::args_os().nth(1);
    if arg.as_ref().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    // Fill in variables from a .env file, if there is one. Variables already
    // set in the environment are left as they are.
    if let Err(e) = dotenvy::dotenv()
        && !e.not_found()
    {
        eprintln!("doc-mcp: could not read .env: {e}");
        return ExitCode::FAILURE;
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
