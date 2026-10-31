//! Text the model writes for the person, without em or en dashes (the copy rules: Klipolo never shows one).

use klipolo_chat::sanitize::reply;

#[test]
fn dashes_become_commas_or_hyphens() {
    for (given, shown) in [
        // Between clauses, spaced or not: a comma.
        (
            "Done \u{2014} want captions too?",
            "Done, want captions too?",
        ),
        ("Done\u{2014}want more?", "Done, want more?"),
        ("Which part \u{2015} the intro?", "Which part, the intro?"),
        ("Cut it \u{2013} or keep it?", "Cut it, or keep it?"),
        // Between digits, and inside a word: a hyphen.
        ("Cut 10\u{2013}20 s.", "Cut 10-20 s."),
        ("Cut 10 \u{2013} 20 s.", "Cut 10-20 s."),
        ("Call 555\u{2012}0100", "Call 555-0100"),
        ("a pre\u{2013}roll ad", "a pre-roll ad"),
        // At either end it goes; after other punctuation it leaves a space.
        ("\u{2014} Sure.", "Sure."),
        ("Sure \u{2014}", "Sure"),
        ("Wait, \u{2014} ok", "Wait, ok"),
        // Nothing else changes.
        ("No dashes - here.", "No dashes - here."),
        ("", ""),
    ] {
        assert_eq!(reply(given), shown, "{given:?}");
    }
}
