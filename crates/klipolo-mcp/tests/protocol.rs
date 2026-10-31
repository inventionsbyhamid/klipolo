//! The protocol core, on a `FakeHost` holding the chat crate's two-clip fixture.

#[path = "support/fake_host.rs"]
mod fake_host;

use std::path::Path;
use std::sync::Arc;

use fake_host::FakeHost;

#[test]
fn crate_has_no_vendor_dependency() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("the manifest");
    let lines: Vec<&str> = (manifest.lines())
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect();
    assert!(lines.iter().any(|l| l.starts_with("klipolo-chat")));
    for vendor in [
        "klipolo-meta",
        "klipolo-analysis",
        "klipolo_meta",
        "klipolo_analysis",
    ] {
        assert!(
            !lines.iter().any(|l| l.contains(vendor)),
            "{vendor} in Cargo.toml"
        );
    }
    let _: Arc<dyn klipolo_mcp::host::Host> = FakeHost::empty();
}
