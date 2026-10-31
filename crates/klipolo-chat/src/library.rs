//! The media the person can add by chat: the project's own music, sounds and pictures and Klipolo's
//! bundled pack, each by name and kind. The app lists them (its library's tabs say which file is music and which a
//! sound); the context shows them as `## Library` and the media tools name them.
//!
//! A name is found leniently but never guessed (`Library::find`): case, a file's extension and punctuation don't count
//! ("soft-ambient" is "Soft ambient", "lofi" is "lofi.mp3"), then a name holding the words asked for, or held in them
//! ("lofi" is "lofi-beat.mp3"), when only one does. "The song" and the like (`is_generic`) are the person's one music
//! file, when they have exactly one.

use klipolo_model::project::Asset;

/// What a library item is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// A song to play under the video (`addMusic`).
    Music,
    /// A sound effect at a moment (`addSound`).
    Sound,
    /// A picture over the video, a sticker included (`addImage`).
    Image,
}

impl MediaKind {
    /// How a refusal names one ("no sound in Library is called ...").
    pub(crate) fn noun(self) -> &'static str {
        match self {
            MediaKind::Music => "music",
            MediaKind::Sound => "sound",
            MediaKind::Image => "picture",
        }
    }
}

/// One file the person can add.
#[derive(Clone, Debug, PartialEq)]
pub struct LibraryItem {
    /// As the library shows it and the model names it ("lofi-beat.mp3", "Whoosh").
    pub name: String,
    pub kind: MediaKind,
    /// The file. The project's own asset, or for the pack one the app made for the bundled file (a stable id): the ops
    /// that place it bring it into the project when the project doesn't have it yet.
    pub asset: Asset,
    /// From Klipolo's bundled pack, not the person's own.
    pub pack: bool,
}

/// The project's own media and the pack, in the order the app lists them (the context keeps it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    pub items: Vec<LibraryItem>,
}

/// What a name found.
#[derive(Debug, PartialEq)]
pub(crate) enum Found<'a> {
    One(&'a LibraryItem),
    None,
    /// More than one item fits as well as the others.
    Several(Vec<&'a LibraryItem>),
}

/// Words that point at a song without naming one: "the song", "my music", "the track".
const GENERIC: [&str; 9] = [
    "song",
    "songs",
    "music",
    "track",
    "tune",
    "audio",
    "soundtrack",
    "background",
    "file",
];
/// Words around them that don't name anything.
const FILLER: [&str; 11] = [
    "the", "my", "our", "this", "that", "a", "an", "your", "her", "his", "their",
];

/// `name` as names are compared: lowercase, without a file's extension, punctuation read as a space, one space
/// between words.
pub(crate) fn plain(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let stem = match lower.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic()) =>
        {
            stem
        }
        _ => lower.as_str(),
    };
    (stem.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `asked` points at a song without naming it ("the song", "my music").
pub(crate) fn is_generic(asked: &str) -> bool {
    let plain = plain(asked);
    let words: Vec<&str> = plain.split(' ').filter(|w| !FILLER.contains(w)).collect();
    !words.is_empty() && words.iter().all(|w| GENERIC.contains(w))
}

impl Library {
    /// The items of `kind` as listed, or only the person's own.
    pub(crate) fn of_kind(&self, kind: MediaKind, own_only: bool) -> Vec<&LibraryItem> {
        (self.items.iter())
            .filter(|i| i.kind == kind && !(own_only && i.pack))
            .collect()
    }

    /// The item `asked` names among `kinds`, tried in that order: one whose name reads the same (the person's own over
    /// the pack's when both do), else the one name holding those words or held in them.
    pub(crate) fn find(&self, asked: &str, kinds: &[MediaKind]) -> Found<'_> {
        let want = plain(asked);
        if want.is_empty() {
            return Found::None;
        }
        let words = |s: &str| format!(" {s} ");
        for &kind in kinds {
            let items = self.of_kind(kind, false);
            let same: Vec<&LibraryItem> = (items.iter().copied())
                .filter(|i| plain(&i.name) == want)
                .collect();
            let own: Vec<&LibraryItem> = same.iter().copied().filter(|i| !i.pack).collect();
            match (same.as_slice(), own.as_slice()) {
                ([one], _) | (_, [one]) => return Found::One(one),
                ([], _) => {}
                _ => return Found::Several(same),
            }
            let near: Vec<&LibraryItem> = (items.iter().copied())
                .filter(|i| {
                    let name = plain(&i.name);
                    !name.is_empty()
                        && (words(&name).contains(&words(&want))
                            || words(&want).contains(&words(&name)))
                })
                .collect();
            match near.as_slice() {
                [one] => return Found::One(one),
                [] => {}
                _ => return Found::Several(near),
            }
        }
        Found::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_plainly() {
        assert_eq!(plain("Lofi-Beat.MP3"), "lofi beat");
        assert_eq!(plain("soft-ambient"), "soft ambient");
        assert_eq!(plain("  Speech  bubble "), "speech bubble");
        // Not an extension: digits only, or too long.
        assert_eq!(plain("take 1.5"), "take 1 5");
        assert_eq!(plain("notes.longextension"), "notes longextension");
        assert!(is_generic("the song") && is_generic("My Music") && is_generic("song"));
        assert!(!is_generic("the lofi song") && !is_generic("the") && !is_generic(""));
    }
}
