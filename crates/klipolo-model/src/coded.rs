//! What Rust says to people: a stable code plus the values its message needs, worded by the UI in the
//! person's language (`src/i18n/codes.ts`). Rust never sends English for the UI to show. Each crate keeps a table of
//! its codes with their English (`Messages`), used for logs, MCP replies and the bench, and as the source text of the
//! UI's catalogue (the descriptors are generated from `src/bindings/*Codes.json`). `coded_msg!` refuses to compile
//! a code its table doesn't have.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contrib::{Catalog, ContribId};
use crate::time::{clock, Us};

/// A user-facing message as a code and its values: `{ code: "model.notFound.clip", params: {} }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct CodedError {
    pub code: String,
    pub params: BTreeMap<String, ErrorParam>,
}

/// A value a message shows. Names ending in `Us` are times (worded as a clock, "1:05"), and names ending in
/// `DurationUs` (`maxDurationUs`) short lengths (worded in seconds, "2.0 s"); a bare `durationUs` is a video's length
/// and reads as a clock ("Your video is only 3:00 long"). A text that is a contribution id is worded as the
/// contribution's name; a nested code is worded the same way (a clip named "Recording 2").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(untagged)]
#[ts(export)]
pub enum ErrorParam {
    Number(f64),
    Text(String),
    Coded(Box<CodedError>),
}

impl From<&str> for ErrorParam {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}
impl From<String> for ErrorParam {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}
impl From<f64> for ErrorParam {
    fn from(n: f64) -> Self {
        Self::Number(n)
    }
}
impl From<i64> for ErrorParam {
    fn from(n: i64) -> Self {
        Self::Number(n as f64)
    }
}
impl From<u32> for ErrorParam {
    fn from(n: u32) -> Self {
        Self::Number(f64::from(n))
    }
}
impl From<usize> for ErrorParam {
    fn from(n: usize) -> Self {
        Self::Number(n as f64)
    }
}
impl From<CodedError> for ErrorParam {
    fn from(e: CodedError) -> Self {
        Self::Coded(Box::new(e))
    }
}

impl CodedError {
    /// The code with no values.
    pub fn new(code: &str) -> Self {
        Self {
            code: code.into(),
            params: BTreeMap::new(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<ErrorParam>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }
}

/// Something said to a person: Rust's own words as a code, or text that is already the person's or the model's (a
/// project name, the planner's summary, question or reason, which stay English: chat is English-only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum Said {
    Coded(CodedError),
    Text(String),
}

impl Said {
    pub fn code(code: &str) -> Self {
        Self::Coded(CodedError::new(code))
    }
}

impl From<CodedError> for Said {
    fn from(e: CodedError) -> Self {
        Self::Coded(e)
    }
}
impl From<String> for Said {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}
impl From<&str> for Said {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}

/// One crate's codes with their English: `("model.layer.tooShort", "A layer is too short.")`. `{name}` marks a value.
pub type Messages = &'static [(&'static str, &'static str)];

/// Whether `table` has `code`; `const`, so the macros check every code when the crate compiles.
pub const fn has(table: Messages, code: &str) -> bool {
    let code = code.as_bytes();
    let mut i = 0;
    while i < table.len() {
        let candidate = table[i].0.as_bytes();
        if candidate.len() == code.len() {
            let mut j = 0;
            while j < code.len() && candidate[j] == code[j] {
                j += 1;
            }
            if j == code.len() {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// `e` in English, from the first table that has its code. Logs, MCP replies and the bench read this; the UI never
/// shows it. An unknown code comes back as it is.
pub fn english(tables: &[Messages], e: &CodedError) -> String {
    let lookup = |code: &str| {
        (tables.iter().flat_map(|t| t.iter()))
            .find(|(c, _)| *c == code)
            .map(|(_, m)| *m)
    };
    render(e, &lookup)
}

/// `e` worded from `lookup` (a code → its template with `{name}` placeholders), nested codes too, with every value as
/// `english` writes it (`…Us` a clock, `…DurationUs` seconds, `…AtMs` a moment, `…Bytes` a file size, contribution
/// ids by name, a `language` by its name). An unknown code comes back as it is. `english` is this over the catalogue tables; the
/// recorder's native menus use it with the person's own words (klipolo-capture `shell_view::Words`), so the two never
/// word a value differently.
pub fn render<'a>(e: &CodedError, lookup: &dyn Fn(&str) -> Option<&'a str>) -> String {
    let Some(template) = lookup(&e.code) else {
        return e.code.clone();
    };
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(len) = rest[open..].find('}') else {
            break;
        };
        let name = &rest[open + 1..open + len];
        match e.params.get(name) {
            Some(ErrorParam::Number(n)) if name.ends_with("DurationUs") => {
                out.push_str(&seconds(*n as Us))
            }
            Some(ErrorParam::Number(n)) if name.ends_with("Us") => out.push_str(&clock(*n as Us)),
            Some(ErrorParam::Number(n)) if name.ends_with("AtMs") => {
                out.push_str(&moment(*n as i64))
            }
            Some(ErrorParam::Number(n)) if name.ends_with("Bytes") => out.push_str(&file_size(*n)),
            Some(ErrorParam::Number(n)) => out.push_str(&number(*n)),
            // A language is a Whisper code ("de"): its English name, as the UI names it in the person's language.
            Some(ErrorParam::Text(t)) if name == "language" => {
                out.push_str(crate::languages::english_name(t).unwrap_or(t.as_str()))
            }
            Some(ErrorParam::Text(t)) => out.push_str(&contribution_or_text(t)),
            Some(ErrorParam::Coded(inner)) => out.push_str(&render(inner, lookup)),
            None => out.push_str(&rest[open..=open + len]),
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out
}

/// A file size (`…Bytes`), as the UI's `formatFileSize` writes it: "190 MB", "3.2 GB" (decimal units). Megabytes round
/// up, to tens from 100 MB, and never show under 1 MB.
fn file_size(bytes: f64) -> String {
    if bytes >= 1e9 {
        return format!("{:.1} GB", bytes / 1e9);
    }
    let mb = bytes / 1e6;
    let shown = if mb >= 100.0 {
        (mb / 10.0).ceil() * 10.0
    } else {
        mb.ceil().max(1.0)
    };
    format!("{shown} MB")
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{}", (n * 10.0).round() / 10.0)
    }
}

/// A moment (`…AtMs`, milliseconds since 1970), in UTC to the minute: "2026-10-15 09:41 UTC". The UI says it in the
/// person's time zone and language.
fn moment(ms: i64) -> String {
    let (days, minutes) = (
        ms.div_euclid(86_400_000),
        ms.rem_euclid(86_400_000) / 60_000,
    );
    // Days since 1970 to a civil date (Howard Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        minutes / 60,
        minutes % 60
    )
}

/// A length in seconds to a tenth: "2.0 s".
fn seconds(us: Us) -> String {
    format!("{:.1} s", us as f64 / 1_000_000.0)
}

fn contribution_or_text(t: &str) -> String {
    if !t.starts_with("klipolo.builtin/") {
        return t.to_owned();
    }
    Catalog::builtin()
        .get(&ContribId::from(t))
        .map_or_else(|| t.to_owned(), |d| d.name.clone())
}

/// `CodedError` for `code` with `key = value` params, refusing to compile when `table` has no English for `code`.
#[macro_export]
macro_rules! coded_msg {
    ($table:path, $code:literal $(, $key:ident = $value:expr)* $(,)?) => {{
        const _: () = assert!($crate::coded::has($table, $code), concat!("no English for the code ", $code));
        $crate::coded::CodedError::new($code)$(.with(stringify!($key), $value))*
    }};
}

/// The code itself (`&'static str`), checked like `coded_msg!`: for messages built only when needed (validation runs
/// on every edit and every dry-run frame of a drag).
#[macro_export]
macro_rules! coded_code {
    ($table:path, $code:literal) => {{
        const _: () = assert!(
            $crate::coded::has($table, $code),
            concat!("no English for the code ", $code)
        );
        $code
    }};
}
