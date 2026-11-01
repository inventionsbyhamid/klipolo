//! Reading the crate's own sources (and the workspace's) as text, for the source scans (`tests/consent_scan.rs`, which
//! includes this file by path so it links nothing of `common`'s, FFmpeg included).

use std::path::{Path, PathBuf};

/// klipolo-capture's `src/`.
pub fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub fn read(file: &str) -> String {
    std::fs::read_to_string(src().join(file)).unwrap_or_else(|e| panic!("src/{file}: {e}"))
}

/// Every `.rs` file under klipolo-capture's `src/`, as (its path from `src/` with `/`, its text).
pub fn sources() -> impl Iterator<Item = (String, String)> {
    rs_files(&src()).into_iter()
}

/// Every `.rs` file under `dir`, sorted, as (its path from `dir` with `/`, its text).
pub fn rs_files(dir: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = vec![];
    walk(dir, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let rel = path.strip_prefix(dir).unwrap();
            let rel: Vec<_> = rel
                .iter()
                .map(|c| c.to_string_lossy().into_owned())
                .collect();
            (rel.join("/"), std::fs::read_to_string(&path).unwrap())
        })
        .collect()
}

/// `code` names `word` as a whole identifier: `begin_capture` but not `begin_capture_options`.
pub fn names_word(code: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(word).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// The code of a Rust file without its `//` comments, so docs that explain these rules don't trip them. A `//` inside
/// a string on its line is kept.
pub fn code_of(text: &str) -> String {
    fn strip(line: &str) -> &str {
        let (mut quoted, mut escaped) = (false, false);
        for (i, c) in line.char_indices() {
            match c {
                '\\' if quoted => escaped = !escaped,
                '"' if !escaped => quoted = !quoted,
                '/' if !quoted && line[i + 1..].starts_with('/') => return &line[..i],
                _ => {}
            }
            if c != '\\' {
                escaped = false;
            }
        }
        line
    }
    text.lines().map(strip).collect::<Vec<_>>().join("\n")
}

/// The first line of code in the body of the function whose signature starts with `sig`.
pub fn first_statement<'a>(code: &'a str, sig: &str) -> &'a str {
    let at = code.find(sig).unwrap_or_else(|| panic!("no {sig}"));
    let body = &code[at..];
    let open = body.find('{').unwrap();
    body[open + 1..]
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}
