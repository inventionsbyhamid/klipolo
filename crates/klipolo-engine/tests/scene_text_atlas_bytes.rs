//! What a scene text's atlas holds, on the CPU through `TextProbe`, as the text build thread lays it
//! out: a letter split title draws each distinct glyph once (repeated letters share a cell), and an emoji's colour takes
//! only its own texels, not a plane over the whole atlas.

use klipolo_engine::render::scene::TextProbe;

/// A scene of one text node `text`, split by letter (`split`) or not, at `size` 1080p pixels over most of the frame.
fn scene(text: &str, split: bool, size: u32) -> String {
    let split = if split { r#","split":"chars""# } else { "" };
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"id":"t","type":"text","text":"{text}","box":[0.02,0.02,0.96,0.96],
          "size":{size},"weight":800,"fill":"#FFFFFF"{split}}}]}}"##
    )
}

/// The text's atlas bytes and whether it was drawn softer than its size, at 4K (`px` 2) or 1080p.
fn atlas(json: &str, uhd: bool) -> (u64, bool) {
    let (size, px) = if uhd {
        ((3840, 2160), 2.0)
    } else {
        ((1920, 1080), 1.0)
    };
    let probe = TextProbe::new(json, &Default::default(), size, px).unwrap();
    (probe.atlas_bytes(), probe.softer())
}

/// 480 letters split by letter at 150 px on a 4K frame, eight letters over and over, take about what sixteen of
/// them take (each letter at each subpixel offset drawn once), and are drawn sharp. Each its own cell, they were past
/// the atlas's budget and drawn softer.
#[test]
fn repeated_letters_share_their_cells() {
    let (few, _) = atlas(&scene(&"ABCDEFGH".repeat(2), true, 150), true);
    let (many, softer) = atlas(&scene(&"ABCDEFGH".repeat(60), true, 150), true);
    println!("16 letters: {few} bytes; 480 letters: {many} bytes");
    assert!(!softer, "480 letters drawn sharp: {many} bytes");
    assert!(
        many < 3 * few,
        "480 letters {many} bytes against 16's {few}"
    );
}

/// One emoji in a line takes colour texels only where it is drawn: a line with an emoji holds a little more than
/// the same line without it, where a colour plane over the whole atlas held three times as much. (Where this computer
/// has no colour emoji face, the emoji draws as an outline glyph and there is nothing to measure.)
#[test]
fn an_emojis_colour_takes_only_its_own_texels() {
    let (plain, _) = atlas(&scene("Ana XX Lee", false, 64), false);
    let (emoji, _) = atlas(&scene("Ana ❤️ Lee", false, 64), false);
    println!("\"Ana XX Lee\": {plain} bytes; \"Ana ❤️ Lee\": {emoji} bytes");
    if emoji * 50 <= plain * 51 {
        println!("no colour emoji face here: nothing drawn in colour");
        return;
    }
    assert!(
        emoji * 10 < plain * 18,
        "an emoji's colour: {emoji} bytes against {plain} without it"
    );
}
