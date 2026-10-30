//! The text a definition's settings carry: hidden characters are refused at load,
//! never cleaned, and real text in any script, emoji sequences included, loads.

use klipolo_model::contrib::Provenance;

use super::*;

/// Setting text an agent wrote (a label, a choice's option) with control, invisible or direction
/// characters is refused at load, never cleaned (cleaning it would change the key); and so is a `basedOn` that isn't
/// a well-formed id.
#[test]
fn hidden_characters_in_setting_text_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let cases = [
        (
            "generator.label.v1",
            r#"[{"key":"speed","label":"Sp\u202eeed","type":"float","min":0,"max":4,"default":1}]"#,
        ),
        (
            "generator.option.v1",
            r#"[{"key":"mode","label":"Mode","type":"choice","options":["a","b\u200b"],"default":"a"}]"#,
        ),
        (
            "generator.newline.v1",
            r#"[{"key":"speed","label":"Sp\need","type":"float","min":0,"max":4,"default":1}]"#,
        ),
        // A line separator, a run of variation selectors, a font default's family.
        (
            "generator.separator.v1",
            r#"[{"key":"speed","label":"Sp\u2028eed","type":"float","min":0,"max":4,"default":1}]"#,
        ),
        (
            "generator.selectors.v1",
            r#"[{"key":"speed","label":"Glow \u2764\ufe0f\ufe0f","type":"float","min":0,"max":4,"default":1}]"#,
        ),
        (
            "generator.font.v1",
            r#"[{"key":"font","label":"Font","type":"font","default":{"family":"Inter\u202e","source":{"type":"bundled"}}}]"#,
        ),
    ];
    for (local, params) in cases {
        let def = ContribDef {
            params: serde_json::from_str(params).unwrap(),
            ..waves(local)
        };
        let dir = version(root.path(), local, &def, BODY);
        let refused = load(&dir, local).map(|_| ()).unwrap_err();
        assert!(
            refused.to_string().contains("setting"),
            "{local}: {refused}"
        );
    }
    let local = "generator.based.v1";
    let def = ContribDef {
        provenance: Some(Provenance {
            made_by: "Example Client".into(),
            created_at_ms: 1,
            based_on: Some(ContribId::from("../x")),
            license: "original".into(),
        }),
        ..waves(local)
    };
    let dir = version(root.path(), local, &def, BODY);
    assert!(matches!(
        load(&dir, local),
        Err(PackError::NotClean("provenance"))
    ));
}

/// Real text in settings loads: emoji with a variation selector or joined by ZWJ, a keycap,
/// a Persian word with a ZWNJ inside it, CJK, Arabic and accented Latin.
#[test]
fn real_text_in_settings_loads() {
    let root = tempfile::tempdir().unwrap();
    let local = "generator.text.v1";
    let def = ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"glow","label":"\u2764\ufe0f Glow","type":"float","min":0,"max":4,"default":1},
                {"key":"coder","label":"\ud83d\udc69\u200d\ud83d\udcbb Coder","type":"float","min":0,"max":4,"default":1},
                {"key":"first","label":"1\ufe0f\u20e3 First","type":"float","min":0,"max":4,"default":1},
                {"key":"mode","label":"\u0645\u06cc\u200c\u0634\u0648\u062f","type":"choice","options":["\u6771\u4eac","\u0645\u0631\u062d\u0628\u0627","Cr\u00e8me"],"default":"Cr\u00e8me"}]"#,
        )
        .unwrap(),
        ..waves(local)
    };
    let dir = version(root.path(), local, &def, BODY);
    let loaded = load(&dir, local);
    assert!(loaded.is_ok(), "{:?}", loaded.err());
}
