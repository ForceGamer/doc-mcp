//! Drives the built server over stdio the way an MCP client does.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn serves_the_three_tools_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Inbox.md", "inbox text");
    write(dir.path(), "Projects/Plan.md", "the plan");
    write(dir.path(), "Daily/2026/11/03-TUE/Meeting.md", "met people");

    let mut server = Command::new(env!("CARGO_BIN_EXE_doc-mcp"))
        .arg(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = BufReader::new(server.stdout.take().unwrap());

    let mut send = |message: &str| writeln!(stdin, "{message}").unwrap();
    let mut reply = |id: u32| {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(
            line.contains(&format!("\"id\":{id},")),
            "reply {id}: {line}"
        );
        line
    };

    send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    assert!(reply(1).contains(r#""name":"doc-mcp""#));
    send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);

    send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = reply(2);
    for tool in ["list_notes", "read_note", "read_all_notes"] {
        assert!(tools.contains(&format!("\"name\":\"{tool}\"")), "{tools}");
    }

    send(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_notes","arguments":{"tree":true}}}"#,
    );
    let listing = reply(3);
    assert!(
        listing.contains("Plan.md (md)") && !listing.contains("Daily/"),
        "{listing}"
    );

    send(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_note","arguments":{"path":"2026-11-03/Meeting.md"}}}"#,
    );
    let note = reply(4);
    assert!(
        note.contains("met people") && note.contains(r#""isError":false"#),
        "{note}"
    );

    send(
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"read_all_notes","arguments":{"path":"Daily"}}}"#,
    );
    assert!(reply(5).contains(r#""isError":true"#));

    // `path` is accepted for the directory, as the other two tools call it;
    // any other unknown argument is an error instead of a silent root listing.
    send(
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"list_notes","arguments":{"path":"Projects"}}}"#,
    );
    assert!(reply(6).contains("Contents of Projects:"));
    send(
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_notes","arguments":{"folder":"Projects"}}}"#,
    );
    let refused = reply(7);
    assert!(
        refused.contains("folder") && !refused.contains("Contents of"),
        "{refused}"
    );

    // Closing stdin ends the session.
    drop(stdin);
    assert!(server.wait().unwrap().success());
}

/// Start the server with no argument in `cwd` and return what it printed to
/// stderr once stdin is closed.
fn startup_message(cwd: &Path, notes_dir_env: Option<&Path>) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_doc-mcp"));
    command
        .current_dir(cwd)
        .env_remove("NOTES_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(dir) = notes_dir_env {
        command.env("NOTES_DIR", dir);
    }
    let output = command.output().unwrap();
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn reads_the_notes_dir_from_a_dotenv_file() {
    let notes = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    fs::write(
        cwd.path().join(".env"),
        format!("# notes\nNOTES_DIR=\"{}\"\n", notes.path().display()),
    )
    .unwrap();

    let message = startup_message(cwd.path(), None);
    assert!(
        message.contains(&format!("serving {}", notes.path().display())),
        "{message}"
    );

    // A variable already in the environment wins over .env.
    let message = startup_message(cwd.path(), Some(other.path()));
    assert!(
        message.contains(&format!("serving {}", other.path().display())),
        "{message}"
    );
}
