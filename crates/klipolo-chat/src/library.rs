//! The media the person can add by chat: the project's own music, sounds and pictures and Klipolo's
//! bundled pack, each by name and kind. The app lists them (its library's tabs say which file is music and which a
//! sound); the context shows them as `## Library` and the media tools name them.
//!
//! A name is found leniently but never guessed (`Library::find`): case, a file's extension and punctuation don't count
//! ("soft-ambient" is "Soft ambient", "lofi" is "lofi.mp3"), then a name holding the words asked for, or held in them
//! ("lofi" is "lofi-beat.mp3"), when only one does. The last word asked for may be cut short, as Library shows a long
//! name, when at least `MIN_PREFIX` of its characters are left ("holiday in lis" is "Holiday in Lisbon.jpg"). "The
//! song" and the like (`is_generic`) are the person's one music file, when they have exactly one.

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

/// The fewest characters a word cut short may keep and still be read as the start of a name's word.
pub const MIN_PREFIX: usize = 3;

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

/// Whether the words of `name` (`plain`) hold those of `want` in a row; with `cut_short`, the last of them may be the
/// start of the name's word, at least `MIN_PREFIX` characters of it.
fn holds(name: &str, want: &str, cut_short: bool) -> bool {
    let have: Vec<&str> = name.split(' ').collect();
    let words: Vec<&str> = want.split(' ').collect();
    let Some((last, rest)) = words.split_last() else {
        return false;
    };
    if name.is_empty() || want.is_empty() || words.len() > have.len() {
        return false;
    }
    (0..=have.len() - words.len()).any(|i| {
        let word = have[i + rest.len()];
        have[i..i + rest.len()] == *rest
            && (word == *last
                || (cut_short && last.chars().count() >= MIN_PREFIX && word.starts_with(last)))
    })
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
                    holds(&name, &want, true) || holds(&want, &name, false)
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

    fn item(name: &str) -> LibraryItem {
        let asset = serde_json::from_value(serde_json::json!({
            "id": name, "kind": "image", "path": "/x.png", "name": name, "durationUs": 5_000_000,
            "width": 1, "height": 1, "fps": 0.0, "hasAudio": false,
        }))
        .unwrap();
        LibraryItem {
            name: name.into(),
            kind: MediaKind::Image,
            asset,
            pack: false,
        }
    }

    #[test]
    fn a_name_cut_short_as_library_shows_it_is_found() {
        let library = Library {
            items: vec![
                item("Holiday photos from the long weekend in Lisbon.jpg"),
                item("logo.png"),
            ],
        };
        let found = |asked: &str| match library.find(asked, &[MediaKind::Image]) {
            Found::One(i) => Some(i.name.as_str()),
            _ => None,
        };
        let holiday = Some("Holiday photos from the long weekend in Lisbon.jpg");
        // Cut at a word, or inside one with enough of it left.
        assert_eq!(found("Holiday photos from the long weekend in"), holiday);
        assert_eq!(
            found("holiday photos from the long weekend in lis"),
            holiday
        );
        assert_eq!(found("log"), Some("logo.png"));
        // Too little of a word, or a word that doesn't start the name's: none.
        assert_eq!(found("lo"), None);
        assert_eq!(found("holiday photos from the lung"), None);
        // The name held in what was asked takes whole words only.
        assert_eq!(found("our logo please"), Some("logo.png"));
        assert_eq!(found("our logos"), None);
    }
}
