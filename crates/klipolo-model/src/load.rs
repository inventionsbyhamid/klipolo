//! Loading project files. Only the current format opens; files from older test builds are refused with a plain
//! message (pre-release: no migrations).
//!
//! A file is parsed once, straight into [`Project`] (no JSON tree first: that tree peaks at several times the file's
//! size and made opening about 3× slower). Only a file that doesn't load that way — another version, a parse error, a
//! project that doesn't validate — is read again as a tree, by the same rules as before, to say why. So is a file
//! saved before the cursor record, which names a recording's cursor file in a field of its own (`earlier`).

use serde_json::Value;

use crate::contrib::FEATURES;
use crate::error::{ModelError, Result};
use crate::project::{Project, PROJECT_VERSION};
use crate::validate::validate;

mod earlier;

fn parse_error(e: serde_json::Error) -> ModelError {
    ModelError::Parse(e.to_string())
}

/// Parse a project file into a validated current-format project.
pub fn load_project(json: &str) -> Result<Project> {
    if earlier::names_cursor_paths(json) {
        return load_from_tree(json);
    }
    match serde_json::from_str::<Project>(json) {
        Ok(project) if project.version == PROJECT_VERSION && validate(&project).is_ok() => {
            Ok(project)
        }
        // Rare, so the error (or, for a file serde reads only as a tree, the project) comes from the tree.
        _ => load_from_tree(json),
    }
}

/// [`load_project`] by way of a JSON tree: the version and requirements are read first, then the project.
fn load_from_tree(json: &str) -> Result<Project> {
    let value: Value = serde_json::from_str(json).map_err(parse_error)?;
    match value.get("version").and_then(Value::as_u64) {
        Some(v) if v == u64::from(PROJECT_VERSION) => load_current(value),
        Some(v) if v < u64::from(PROJECT_VERSION) => Err(ModelError::OlderVersion),
        Some(other) => Err(ModelError::UnsupportedVersion(
            u32::try_from(other).unwrap_or(u32::MAX),
        )),
        None => Err(ModelError::Parse("missing version".into())),
    }
}

/// Parses and validates a current-format file. When it lists requirements this build lacks and doesn't parse or
/// doesn't validate (a newer Klipolo's value ranges), the error says to update Klipolo instead of naming a parse or
/// validation problem this build can't judge.
fn load_current(mut value: Value) -> Result<Project> {
    earlier::read_cursor_paths(&mut value);
    // Requirements this build doesn't know (an entry that isn't text counts too).
    let newer: Vec<String> = value
        .get("requires")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(|v| match v.as_str() {
                    Some(known) if FEATURES.contains(&known) => None,
                    Some(unknown) => Some(unknown.to_owned()),
                    None => Some(v.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    let from_newer = |error| {
        if newer.is_empty() {
            error
        } else {
            ModelError::NewerFeatures(newer.join(", "))
        }
    };
    let project: Project = serde_json::from_value(value).map_err(|e| from_newer(parse_error(e)))?;
    validate(&project).map_err(from_newer)?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tree path, as `load_project` was before it parsed once.
    fn by_tree(json: &str) -> Result<Project> {
        load_from_tree(json)
    }

    #[test]
    fn parsing_once_loads_what_the_tree_loads() {
        let golden = include_str!("../tests/golden/project-v4.json");
        let mut newer = Project::new("Newer");
        newer.requires.insert("templates@1".into());
        let mut duplicate = serde_json::to_string(&Project::new("First")).unwrap();
        // A repeated key: serde refuses it in a struct, the tree keeps the last one, so it loads as before.
        duplicate.insert_str(1, r#""name":"Dropped","#);
        let cases = [
            golden.to_string(),
            serde_json::to_string(&Project::new("New")).unwrap(),
            serde_json::to_string(&newer).unwrap(),
            duplicate,
        ];
        for json in &cases {
            let loaded = load_project(json).unwrap();
            assert_eq!(loaded, by_tree(json).unwrap());
            // Saving and reopening changes nothing (exact floats, `float_roundtrip`).
            assert_eq!(
                load_project(&serde_json::to_string(&loaded).unwrap()).unwrap(),
                loaded
            );
        }
        // Every refusal comes from the tree, so it reads as before.
        let mut fps = serde_json::to_value(Project::new("Bad")).unwrap();
        fps["settings"]["fps"] = 7.into();
        let mut old = serde_json::to_value(Project::new("Old")).unwrap();
        old["version"] = 1.into();
        for json in [
            "{not json".to_string(),
            "[1, 2]".into(),
            r#"{"name": "No version"}"#.into(),
            old.to_string(),
            fps.to_string(),
        ] {
            assert_eq!(load_project(&json), by_tree(&json));
            assert!(load_project(&json).is_err());
        }
    }
}
