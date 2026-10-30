//! A development spend ledger: with `KLIPOLO_SPEND_LEDGER` set, each
//! successful call whose usage is known appends one CSV row with its cost.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Names the CSV file the ledger appends to. Unset: nothing is written.
pub const SPEND_LEDGER_ENV: &str = "KLIPOLO_SPEND_LEDGER";

const HEADER: &str =
    "time_utc,model,purpose,input_tokens,output_tokens,frames_or_seconds,est_cost_usd";

/// Keeps rows from parallel calls whole, and the header written once.
static WRITING: Mutex<()> = Mutex::new(());

pub(crate) struct Row<'a> {
    pub model: &'a str,
    pub purpose: &'a str,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub frames_or_seconds: String,
    pub cost_usd: f64,
}

/// Appends `row`, with the header first when the file is new or empty. A
/// ledger that cannot be written never fails the call it records.
pub(crate) fn append(path: &Path, row: &Row) {
    let _writing = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(error) = try_append(path, row) {
        eprintln!(
            "klipolo-meta: could not write the spend ledger {}: {error}",
            path.display()
        );
    }
}

fn try_append(path: &Path, row: &Row) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut text = String::new();
    if file.metadata()?.len() == 0 {
        text.push_str(HEADER);
        text.push('\n');
    }
    let count = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_default();
    let fields = [
        utc(SystemTime::now()),
        field(row.model),
        field(row.purpose),
        count(row.input_tokens),
        count(row.output_tokens),
        field(&row.frames_or_seconds),
        format!("{:.6}", row.cost_usd),
    ];
    text.push_str(&fields.join(","));
    text.push('\n');
    file.write_all(text.as_bytes())
}

/// A CSV field, quoted when it needs to be.
fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// `2026-10-30T10:47:32Z`.
fn utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Year, month and day of a count of days since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`, for dates after 1970).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn utc_formats_known_instants() {
        let at = |s| utc(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_793_357_252), "2026-10-30T10:47:32Z");
    }

    #[test]
    fn fields_with_commas_or_quotes_are_quoted() {
        assert_eq!(field("responses"), "responses");
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
