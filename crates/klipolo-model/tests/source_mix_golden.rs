//! The page pins a kind of recorded sound's mix ops (a sound lane's Mute and Volume, the Centre switch, Fill gaps) to the
//! ops chat's source tools compile to, in `apps/desktop/src/features/timeline/keys/source-mix-parity.json`. The Rust
//! test that writes the file lands with those tools; until then this one holds the file to the model: each of its
//! projects loads, and every case's ops apply to its project as one batch.

use std::path::PathBuf;

use klipolo_model::{apply_ops, Op, Project};
use serde_json::Value;

fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/features/timeline/keys/source-mix-parity.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("the golden is JSON")
}

fn project(part: &Value) -> Project {
    serde_json::from_value(part["project"].clone()).expect("a project the model reads")
}

/// `ops` applied to `p` as one batch: refused with the model's reason, or the project after them.
fn applies(p: &Project, ops: &Value, what: &str) -> Project {
    let ops: Vec<Op> =
        serde_json::from_value(ops.clone()).unwrap_or_else(|e| panic!("{what}: {e}"));
    apply_ops(p, &ops).unwrap_or_else(|e| panic!("{what}: {e:?}"))
}

#[test]
fn every_pinned_case_s_ops_apply_to_its_project() {
    let golden = golden();
    let mut checked = 0;
    for part in [&golden, &golden["twoMics"], &golden["cameraSound"]] {
        let p = project(part);
        for case in part["cases"].as_array().expect("cases") {
            applies(&p, &case["ops"], &case["call"].to_string());
            checked += 1;
        }
    }
    let camera = &golden["cameraSound"];
    let p = project(camera);
    for name in ["fill", "fillOn"] {
        applies(&p, &camera[name]["ops"], name);
        checked += 1;
    }
    let muted = &camera["fillMuted"];
    applies(&project(muted), &muted["ops"], "fillMuted");
    checked += 1;
    // 8 requests on the two takes, 2 lanes of the two mics, 3 on the camera sound, and the three Fill gaps cases.
    assert_eq!(checked, 16);
}

/// A lane's Mute on the two takes reaches both takes' mics and the clip with a mix of its own, and leaves the clip whose
/// mic plays from its own layer as separating left it.
#[test]
fn the_mic_lane_s_mute_mutes_every_take_s_mic_but_the_separated_part() {
    let golden = golden();
    let p = project(&golden);
    let case = golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| {
            c["call"]["name"] == "muteSource"
                && c["call"]["args"]["source"] == "mic"
                && c["call"]["args"]["muted"] == true
        })
        .expect("muting the mic");
    let after = applies(&p, &case["ops"], "mute the mic");
    for r in &after.recordings {
        let mic = r
            .sources
            .iter()
            .find(|s| s.id == "mic")
            .expect("each take has a mic");
        assert!(mic.mix.muted, "{}'s mic", r.id);
    }
    let own = |id: &str| {
        after
            .timeline
            .main
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .source_mix["mic"]
            .clone()
    };
    assert!(own("c2").muted, "c2's own mix is muted too");
    let before = |id: &str| {
        p.timeline
            .main
            .iter()
            .find(|c| c.id == id)
            .unwrap()
            .source_mix["mic"]
            .clone()
    };
    assert_eq!(
        own("c1"),
        before("c1"),
        "the separated part is left as it was"
    );
}
