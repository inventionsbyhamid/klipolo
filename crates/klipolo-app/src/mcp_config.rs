//! What Settings' "Copy config" puts on the clipboard for an MCP app: one `mcpServers` JSON block
//! such apps take, starting this same Klipolo with `--mcp` (the stdio bridge, `klipolo_mcp::bridge`). And whether
//! Klipolo has to move first: run from its disk image, or from where macOS translocated it, its path is gone once it
//! quits, and so is the app's way in.

use std::path::Path;

use serde::Serialize;

#[derive(Serialize)]
struct Config<'a> {
    #[serde(rename = "mcpServers")]
    mcp_servers: Servers<'a>,
}

#[derive(Serialize)]
struct Servers<'a> {
    klipolo: Server<'a>,
}

/// In this order: the command, then its arguments.
#[derive(Serialize)]
struct Server<'a> {
    command: &'a str,
    args: [&'a str; 1],
}

/// `{"mcpServers": {"klipolo": {"command": "<exe>", "args": ["--mcp"]}}}`, pretty-printed with two spaces; `exe` is
/// the running executable (`std::env::current_exe`).
pub fn config_json(exe: &Path) -> String {
    let command = exe.to_string_lossy();
    let config = Config {
        mcp_servers: Servers {
            klipolo: Server {
                command: &command,
                args: ["--mcp"],
            },
        },
    };
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// Whether Klipolo runs from where it won't stay: a disk image (`/Volumes/...`) or a translocated copy
/// (`.../AppTranslocation/...`, made by macOS for an app opened where it was downloaded).
pub fn needs_move(exe: &Path) -> bool {
    let path = exe.to_string_lossy();
    path.starts_with("/Volumes/") || path.contains("/AppTranslocation/")
}
