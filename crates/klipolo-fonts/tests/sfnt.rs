fn bundled(file: &str) -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/fonts")
            .join(file),
    )
    .unwrap()
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// A two-face `.ttc` built from two bundled TTFs: a `ttcf` header whose two offsets point at copies of each file, each
/// copy's table records re-pointed at offsets from the start of the collection (as a collection's are).
fn two_face_collection() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let inter = bundled("Inter/Inter-Regular.ttf");
    let anton = bundled("Anton/Anton-Regular.ttf");
    let header = 12 + 4 * 2;
    let first = header;
    let second = (first + inter.len()).next_multiple_of(4);
    let mut ttc = Vec::with_capacity(second + anton.len());
    ttc.extend_from_slice(b"ttcf");
    ttc.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    ttc.extend_from_slice(&2u32.to_be_bytes());
    ttc.extend_from_slice(&(first as u32).to_be_bytes());
    ttc.extend_from_slice(&(second as u32).to_be_bytes());
    for (base, font) in [(first, &inter), (second, &anton)] {
        ttc.resize(base, 0);
        let mut copy = font.clone();
        let tables = u16::from_be_bytes([copy[4], copy[5]]) as usize;
        for t in 0..tables {
            let at = 12 + 16 * t + 8;
            let offset = be32(&copy, at) + base as u32;
            copy[at..at + 4].copy_from_slice(&offset.to_be_bytes());
        }
        ttc.extend_from_slice(&copy);
    }
    (ttc, inter, anton)
}

#[test]
fn a_collection_face_comes_out_as_a_font_that_parses_as_that_face() {
    let (ttc, inter, anton) = two_face_collection();
    let one = klipolo_fonts::sfnt::face_bytes(&ttc, 1).unwrap();
    assert_eq!(
        ttf_parser::Face::parse(&one, 0).unwrap().number_of_glyphs(),
        ttf_parser::Face::parse(&anton, 0)
            .unwrap()
            .number_of_glyphs()
    );
    let zero = klipolo_fonts::sfnt::face_bytes(&ttc, 0).unwrap();
    assert_eq!(
        ttf_parser::Face::parse(&zero, 0)
            .unwrap()
            .number_of_glyphs(),
        ttf_parser::Face::parse(&inter, 0)
            .unwrap()
            .number_of_glyphs()
    );
    // It draws: the extracted face maps letters to the same glyphs as the file it came from.
    let face = ttf_parser::Face::parse(&one, 0).unwrap();
    let original = ttf_parser::Face::parse(&anton, 0).unwrap();
    assert_eq!(face.glyph_index('A'), original.glyph_index('A'));
    assert_eq!(
        klipolo_fonts::sfnt::face_bytes(&ttc, 2),
        Err(klipolo_fonts::sfnt::SfntError::NotAFace(2))
    );
}

#[test]
fn a_plain_font_is_answered_as_it_is_and_bad_offsets_are_refused() {
    let inter = bundled("Inter/Inter-Regular.ttf");
    assert!(matches!(
        klipolo_fonts::sfnt::face_bytes(&inter, 0).unwrap(),
        std::borrow::Cow::Borrowed(_)
    ));
    let mut bad = b"ttcf\0\x01\0\0\0\0\0\x01".to_vec();
    bad.extend_from_slice(&0xFFFF_FF00u32.to_be_bytes());
    assert!(klipolo_fonts::sfnt::face_bytes(&bad, 0).is_err());
    // A table running past the end of the file is refused too, never read.
    let (mut ttc, _, _) = two_face_collection();
    let first = be32(&ttc, 12) as usize;
    let length_at = first + 12 + 12;
    ttc[length_at..length_at + 4].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    assert_eq!(
        klipolo_fonts::sfnt::face_bytes(&ttc, 0),
        Err(klipolo_fonts::sfnt::SfntError::Truncated)
    );
}

/// A face's size is known without copying it, and a face over the cap is refused from its map before
/// any byte is copied; under it, the map gives the same font `face_bytes` does.
#[test]
fn a_face_is_read_from_a_map_and_refused_over_the_cap() {
    use klipolo_fonts::sfnt::{SfntError, face_bytes, face_len, read_face};
    let (ttc, _, anton) = two_face_collection();
    assert_eq!(
        face_len(&ttc, 1).unwrap(),
        face_bytes(&ttc, 1).unwrap().len() as u64
    );
    assert_eq!(face_len(&anton, 0).unwrap(), anton.len() as u64);
    let path = std::env::temp_dir().join(format!("klipolo-sfnt-{}.ttc", std::process::id()));
    std::fs::write(&path, &ttc).unwrap();
    assert_eq!(
        read_face(&path, 1, u64::MAX).unwrap(),
        face_bytes(&ttc, 1).unwrap().into_owned()
    );
    let len = face_len(&ttc, 1).unwrap();
    assert_eq!(read_face(&path, 1, len - 1), Err(SfntError::TooLarge(len)));
    assert_eq!(
        read_face(&path.with_extension("gone"), 0, u64::MAX),
        Err(SfntError::Unreadable)
    );
    std::fs::remove_file(&path).unwrap();
}
