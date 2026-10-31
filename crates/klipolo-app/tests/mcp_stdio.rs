//! `klipolo-app --mcp` is the stdio bridge before anything else starts: no log, no window, nothing on stdout but the
//! bridge's JSON lines, and it ends when its input does. With no connection file, Klipolo counts as off.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[test]
fn mcp_runs_the_stdio_bridge_and_writes_only_json_lines() {
    let support = tempfile::tempdir().unwrap();
    let mut bridge = Command::new(env!("CARGO_BIN_EXE_klipolo-app"))
        .arg("--mcp")
        .env("KLIPOLO_SUPPORT_DIR", support.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "Test", "version": "1"},
    }});
    let ready = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let summary = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "getSummary", "arguments": {},
    }});
    let mut input = bridge.stdin.take().unwrap();
    for msg in [hello, ready, summary] {
        writeln!(input, "{msg}").unwrap();
    }
    drop(input);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = bridge.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = bridge.kill();
            panic!("the bridge didn't end when its input did");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    let mut out = String::new();
    bridge
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let lines: Vec<Value> = (out.lines())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "klipolo");
    assert_eq!(lines[1]["id"], 2);
    assert_eq!(lines[1]["result"]["isError"], true);
    assert_eq!(
        lines[1]["result"]["content"][0]["text"],
        klipolo_mcp::words::OFF
    );
}
