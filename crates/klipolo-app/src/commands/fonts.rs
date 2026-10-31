//! The bundled fonts for the page: the families the Font menu offers, and a face's bytes, so a title typed on the canvas
//! (and measured for its handles) is drawn in the very file the engine draws it with. Only the bundled files are ever
//! read: a family and weight name one of them, never a path.

use klipolo_fonts::bundled::{BUNDLED, BundledFace, bundled_dir, fallback_only};
use serde::Serialize;
use ts_rs::TS;

/// One face of a bundled family.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BundledFont {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// Every face of the families a text can use, in the menu's order (Adobe NotDef, a fallback only, is left out).
#[tauri::command]
pub fn fonts_bundled() -> Vec<BundledFont> {
    BUNDLED
        .iter()
        .filter(|face| !fallback_only(face.family))
        .map(|face| BundledFont {
            family: face.family.into(),
            weight: face.weight,
            italic: face.italic,
        })
        .collect()
}

/// The face of `family` nearest `weight` (the same style first), as CSS and the engine pick one from a family's faces.
pub(crate) fn nearest(family: &str, weight: u16, italic: bool) -> Option<&'static BundledFace> {
    BUNDLED
        .iter()
        .filter(|face| face.family == family && !fallback_only(face.family))
        .min_by_key(|face| (face.italic != italic, face.weight.abs_diff(weight)))
}

/// The bytes of `family`'s face nearest `weight` and `italic`, from the bundled folder.
#[tauri::command(async)]
pub fn fonts_face(
    family: String,
    weight: u16,
    italic: bool,
) -> Result<tauri::ipc::Response, String> {
    let face = nearest(&family, weight, italic).ok_or("That font isn't one of Klipolo's.")?;
    let dir = bundled_dir().ok_or("Klipolo's fonts aren't installed.")?;
    std::fs::read(dir.join(face.file))
        .map(tauri::ipc::Response::new)
        .map_err(|error| {
            tracing::warn!(%error, file = face.file, "a bundled font couldn't be read");
            "Klipolo couldn't read that font. Try again, or reinstall Klipolo.".to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_is_the_nearest_weight_of_the_family() {
        assert_eq!(nearest("Inter", 700, false).unwrap().weight, 700);
        assert_eq!(nearest("Inter", 800, false).unwrap().weight, 700);
        assert_eq!(nearest("Montserrat", 700, false).unwrap().weight, 900);
        assert!(nearest("Adobe NotDef", 400, false).is_none());
        assert!(nearest("../../etc/passwd", 400, false).is_none());
        let families: Vec<String> = fonts_bundled().into_iter().map(|f| f.family).collect();
        assert!(
            families.contains(&"Inter".to_string())
                && !families.contains(&"Adobe NotDef".to_string())
        );
    }
}
