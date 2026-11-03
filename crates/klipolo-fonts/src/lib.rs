//! Klipolo's fonts: the bundled set, aliases, CSS face matching, the one resolver, the fallback chain, the font
//! index with this computer's families, script coverage, the one ranked search, `.ttc` face extraction and the per-run
//! CJK rule, Google's catalogue (a compact snapshot shipped in the app, browsed offline), its downloads and
//! their store, and the fonts a project uses that aren't here.

pub mod alias;
pub mod browse;
pub mod bundled;
pub mod cjk;
pub mod coverage;
pub mod download;
pub mod downloaded;
pub mod english;
pub mod fallback;
pub mod google;
pub mod index;
pub mod installed;
pub mod licence;
pub mod matching;
pub mod missing;
pub mod resolve;
pub mod search;
pub mod sfnt;
pub mod store;
/// A local stand-in for Google's hosts: this crate's download tests and the app's (`test-hosts` only).
#[cfg(feature = "test-hosts")]
pub mod test_server;

/// The script type cosmic-text's `Fallback` names scripts by (the version it resolves), for the engine's chain.
pub use unicode_script;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// `v` with every object's keys sorted, at every depth: a file written from `serde_json::Value`s reads the same
    /// whether or not klipolo-ai's `preserve_order` builds alongside (CI's combined runs).
    pub(crate) fn sorted_keys(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                serde_json::Value::Object(
                    entries
                        .into_iter()
                        .map(|(k, v)| (k, sorted_keys(v)))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(sorted_keys).collect())
            }
            other => other,
        }
    }

    fn repo(path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path)
    }

    /// `pnpm bindings` runs this (its name matches `export_bindings`): `apps/desktop/src/bindings/fontsCodes.json`,
    /// this crate's `fonts.*` codes, their English and their values, for the UI's catalogue (as the app crate writes
    /// `appCodes.json`).
    #[test]
    fn export_bindings_fonts_codes() {
        let list: Vec<serde_json::Value> = crate::english::ENGLISH
            .iter()
            .map(|(code, message)| {
                let params: Vec<&str> = message
                    .split('{')
                    .skip(1)
                    .filter_map(|s| s.split('}').next())
                    .collect();
                serde_json::json!({ "code": code, "message": message, "params": params })
            })
            .collect();
        let path = repo("apps/desktop/src/bindings/fontsCodes.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string_pretty(&list).unwrap() + "\n").unwrap();
    }

    /// `apps/desktop/src/lib/font-fallbacks.json`: the one fallback chain, for every CSS stack the webview builds.
    #[test]
    fn export_bindings_font_fallbacks() {
        let chain = sorted_keys(serde_json::to_value(crate::fallback::chain()).unwrap());
        std::fs::write(
            repo("apps/desktop/src/lib/font-fallbacks.json"),
            serde_json::to_string_pretty(&chain).unwrap() + "\n",
        )
        .unwrap();
    }

    #[test]
    fn every_code_is_in_the_fonts_namespace() {
        for (code, message) in crate::english::ENGLISH {
            assert!(code.starts_with("fonts."), "{code}");
            assert!(!message.trim().is_empty(), "{code}");
        }
        assert_eq!(
            crate::english::english(&crate::english::missing("Avenir Next")),
            "Avenir Next isn't on this computer."
        );
    }
}
