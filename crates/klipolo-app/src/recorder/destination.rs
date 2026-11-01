//! The project a take lands in, open by the time it lands:
//! 1. the project it landed in already (its journal's `Imported` line), when that is the open one: the take isn't
//!    added again there;
//! 2. the project open when the take was armed (`ImportTarget::Project`): the open one, or, when another is open by
//!    now, that project's file, opened again (the open one is saved and closed first, as any opening does);
//! 3. else a new project in the library: a take from the start screen (`ImportTarget::New`), or one whose project's
//!    file is gone. It is named "Recording, {Mon D}" for this computer's date, in the three formats New project makes.
//!
//! The project is opened or made only once the take's files have been read, so a take that failed, or has nothing to
//! edit, leaves the library and the open project as they were. A project made for a take whose step is then refused
//! goes again (`unmake`): no empty project is left behind.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_capture::recover;
use klipolo_capture::session::{ImportTarget, SessionDir};
use klipolo_model::coded::Said;
use klipolo_model::project::FormatKind;

use crate::project::{
    ProjectSessionState, close_in, create_file_in, new_project, open_in, read, with_open,
};

/// A new project's formats, as New project makes them.
pub(super) const FORMATS: [FormatKind; 3] = [
    FormatKind::Landscape,
    FormatKind::Portrait,
    FormatKind::Square,
];

/// A day on this computer's calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Day {
    /// 1 to 12.
    pub(super) month: u32,
    /// 1 to 31.
    pub(super) day: u32,
}

/// Where a take lands.
pub(super) struct Destination<'a> {
    /// The project open when it was armed, or a new one.
    pub(super) target: &'a ImportTarget,
    /// That project's file, as it was open then.
    pub(super) file: Option<&'a Path>,
    /// The library (`~/Movies/Klipolo`), where a new project is made.
    pub(super) library: &'a dyn Fn() -> Result<PathBuf, Said>,
    /// Today, for a new project's name.
    pub(super) today: Day,
}

/// The project a take lands in, open now.
pub(super) struct Target {
    pub(super) id: String,
    /// Where it is saved.
    pub(super) file: PathBuf,
    /// Made for this take just now.
    pub(super) made: bool,
}

impl Target {
    fn there(id: &str, file: &Path) -> Self {
        Self {
            id: id.to_owned(),
            file: file.to_path_buf(),
            made: false,
        }
    }
}

/// The project the take in `dir` lands in (the module doc), open now.
pub(super) fn open_for(
    sessions: &ProjectSessionState,
    dir: &SessionDir,
    to: &Destination<'_>,
) -> Result<Target, Said> {
    let open = open_project(sessions);
    let open_is = |id: &str| open.as_ref().filter(|(open, _)| open == id);
    if let Some(landed) = recover::ending(dir).imported
        && let Some((id, file)) = open_is(&landed)
    {
        return Ok(Target::there(id, file));
    }
    let (name, formats) = match to.target {
        ImportTarget::Project { id } => {
            if let Some((id, file)) = open_is(id) {
                return Ok(Target::there(id, file));
            }
            if let Some(file) = to.file.filter(|file| holds(file, id)) {
                tracing::info!(
                    "recorder: the take's project isn't open now, so it opens again for the take"
                );
                open_in(sessions, file)?;
                return Ok(Target::there(id, file));
            }
            tracing::warn!("recorder: the take's project is gone, so the take lands in a new one");
            (recording_name(to.today), FORMATS.to_vec())
        }
        ImportTarget::NewProject { name, formats } => (name.clone(), formats.clone()),
        ImportTarget::New => (recording_name(to.today), FORMATS.to_vec()),
    };
    let library = (to.library)()?;
    let (snapshot, file) = create_file_in(sessions, &library, new_project(&name, formats))?;
    tracing::info!("recorder: a new project was made for the take");
    Ok(Target {
        id: snapshot.project.id,
        file,
        made: true,
    })
}

/// The project made for a take at `file`, whose step was then refused: closed while it is still the open one, and its
/// file removed, as if it was never made. One that can't be closed stays, with its file.
pub(super) fn unmake(sessions: &ProjectSessionState, file: &Path) {
    let open = with_open(sessions, |open| Ok(open.file == file)).unwrap_or(false);
    if open && let Err(error) = close_in(sessions) {
        tracing::warn!(
            ?error,
            "recorder: the project made for the take couldn't be closed, so it stays"
        );
        return;
    }
    match std::fs::remove_file(file) {
        Ok(()) => tracing::info!("recorder: the project made for the take went again"),
        Err(error) => {
            tracing::warn!(%error, "recorder: the project made for the take couldn't be removed")
        }
    }
}

/// The open project's id and file.
fn open_project(sessions: &ProjectSessionState) -> Option<(String, PathBuf)> {
    with_open(sessions, |open| {
        Ok((open.document.project().id.clone(), open.file.clone()))
    })
    .ok()
}

/// `file` is still project `id`'s.
pub(super) fn holds(file: &Path, id: &str) -> bool {
    read(file).is_ok_and(|project| project.id == id)
}

/// A take's new project: "Recording, Oct 31".
pub(super) fn recording_name(today: Day) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS[today.month.clamp(1, 12) as usize - 1];
    format!("Recording, {month} {}", today.day)
}

/// Today on this computer, in its time zone.
pub(super) fn today() -> Day {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    local_day(now).unwrap_or_else(|| utc_day(now))
}

/// The local date of `secs` since 1970.
#[cfg(unix)]
fn local_day(secs: u64) -> Option<Day> {
    let at = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `tm` is plain data, valid all zeros.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` reads `at` and writes only to `tm`, both ours.
    if unsafe { libc::localtime_r(&at, &mut tm) }.is_null() {
        return None;
    }
    Some(Day {
        month: u32::try_from(tm.tm_mon + 1).ok()?,
        day: u32::try_from(tm.tm_mday).ok()?,
    })
}

#[cfg(not(unix))]
fn local_day(_secs: u64) -> Option<Day> {
    None
}

/// The UTC date of `secs` since 1970 (days to a civil date, after Howard Hinnant's algorithm).
pub(super) fn utc_day(secs: u64) -> Day {
    let days = (secs / 86_400) as i64 + 719_468;
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    Day {
        month: month as u32,
        day: day as u32,
    }
}
