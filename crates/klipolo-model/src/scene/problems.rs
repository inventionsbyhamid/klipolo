//! Collecting problems for the agent: at most [`SceneLimits::MAX_PROBLEMS`], then a count of the rest, so an answer
//! stays small whatever the scene holds. Paths are JSON pointers. Every path and message goes through [`clean`], so
//! text quoted from the scene (keys, values, serde's words) never carries control, bidi or invisible characters, and
//! no problem is longer than its cap.

use super::{SceneLimits, SceneProblem};

/// The longest message and path a problem holds, in characters (escapes count as written).
const MAX_MESSAGE: usize = 600;
const MAX_PATH: usize = 256;

#[derive(Default)]
pub(crate) struct Problems {
    list: Vec<SceneProblem>,
    dropped: usize,
}

impl Problems {
    pub fn push(
        &mut self,
        path: impl Into<String>,
        code: &'static str,
        message: impl Into<String>,
    ) {
        if self.full() {
            self.skip();
            return;
        }
        self.list.push(SceneProblem {
            path: clean(&path.into(), MAX_PATH),
            code,
            message: clean(&message.into(), MAX_MESSAGE),
        });
    }

    /// Whether the list is full: what comes next is only counted, so callers skip wording it.
    pub fn full(&self) -> bool {
        self.list.len() >= SceneLimits::MAX_PROBLEMS
    }

    /// Counts a problem without wording it.
    pub fn skip(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    pub fn len(&self) -> usize {
        self.list.len().saturating_add(self.dropped)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.dropped == 0
    }

    /// Whether a problem already names this exact path.
    pub fn has_path(&self, path: &str) -> bool {
        let path = clean(path, MAX_PATH);
        self.list.iter().any(|p| p.path == path)
    }

    pub fn finish(mut self) -> Vec<SceneProblem> {
        if self.dropped > 0 {
            self.list.push(SceneProblem {
                path: String::new(),
                code: "scene.moreProblems",
                message: format!(
                    "…and {} more problems. Fix these first, then check again.",
                    self.dropped
                ),
            });
        }
        self.list
    }
}

/// `path` with one more step, escaped as a JSON pointer step (`~` → `~0`, `/` → `~1`) and cut to 64 characters.
pub(crate) fn join(path: &str, step: &str) -> String {
    let mut out = String::with_capacity(path.len() + step.len().min(64) + 1);
    out.push_str(path);
    out.push('/');
    for (i, c) in step.chars().enumerate() {
        if i == 64 {
            out.push('…');
            break;
        }
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            c => out.push(c),
        }
    }
    out
}

/// `path` with an index step.
pub(crate) fn at(path: &str, i: usize) -> String {
    format!("{path}/{i}")
}

/// Text from the scene, for a message: at most 40 characters (`clean` escapes what can't be shown).
pub(crate) fn quote(s: &str) -> String {
    let mut out: String = s.chars().take(40).collect();
    if s.chars().nth(40).is_some() {
        out.push('…');
    }
    out
}

/// `s` with control, bidi, zero-width and line or paragraph separator characters written as `\u{…}`, at most `max`
/// characters (diagnostics never echo them raw).
pub(crate) fn clean(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4)));
    let mut shown = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let piece = if hidden(c) {
            format!("\\u{{{:x}}}", u32::from(c))
        } else {
            c.to_string()
        };
        let width = piece.chars().count();
        // Room for this piece, and for the `…` when more follows.
        let room = if chars.peek().is_some() {
            max.saturating_sub(1)
        } else {
            max
        };
        if shown.saturating_add(width) > room {
            out.push('…');
            break;
        }
        shown += width;
        out.push_str(&piece);
    }
    out
}

/// Characters that change or hide how text around them reads: controls, format characters (Cf) and the rest of
/// Unicode's default-ignorable set, tag characters among them (text an agent reads but a person doesn't).
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF0}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}
