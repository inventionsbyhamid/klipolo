//! The licence a Google Fonts download keeps beside its faces (never fetched, so a download asks Google alone, as its
//! consent says). The texts ship in the app (`licences/`):
//! - `OFL-1.1.txt`: the SIL Open Font License 1.1 as the bundled families carry it (Inter's `OFL.txt` without its
//!   copyright line);
//! - `Apache-2.0.txt`: the Apache License 2.0 (Luckiest Guy's `LICENSE.txt`);
//! - `UFL-1.0.txt`: names the Ubuntu Font Licence 1.0 and where its text is, until its full text is added (it needs
//!   one fetch from that page on the build machine).
//!
//! Each family's file starts with its own copyright notice, read from its font (name ID 0), then the licence's text.
//! Its manifest records the licence id (`ofl`, `apache`, `ufl`).

const OFL: &str = include_str!("../licences/OFL-1.1.txt");
const APACHE: &str = include_str!("../licences/Apache-2.0.txt");
const UFL: &str = include_str!("../licences/UFL-1.0.txt");

/// The licence file's name in a family's folder (Google's own names for its three licence folders).
pub fn licence_file(licence: &str) -> &'static str {
    match licence {
        "apache" => "LICENSE.txt",
        "ufl" => "UFL.txt",
        _ => "OFL.txt",
    }
}

/// The licence file's text for `licence` (`ofl`, `apache`, `ufl`), headed by the font's own `copyright` notice when it
/// has one. None for an id the catalogue never gives.
pub fn licence_text(licence: &str, copyright: Option<&str>) -> Option<String> {
    let body = match licence {
        "ofl" => OFL,
        "apache" => APACHE,
        "ufl" => UFL,
        _ => return None,
    };
    Some(match copyright.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => format!("{c}\n\n{body}"),
        None => body.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_of_googles_three_licences_ships() {
        let ofl = super::licence_text("ofl", Some("Copyright 2020 The Testa Authors")).unwrap();
        assert!(ofl.starts_with("Copyright 2020 The Testa Authors\n\n"));
        assert!(ofl.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(
            super::licence_text("apache", None)
                .unwrap()
                .contains("Apache License")
        );
        assert!(
            super::licence_text("ufl", None)
                .unwrap()
                .contains("Ubuntu Font Licence")
        );
        assert_eq!(super::licence_text("gpl", None), None);
    }
}
