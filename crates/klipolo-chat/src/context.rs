//! The project as the language model reads it: one compact text block of five sections, in this order.
//! - `Project`: its name, formats (the primary first), length, captions, and the main track's clips with short refs
//!   (`c1 0.0-41.2 interview.mov`) that tool calls name clips by (`Refs`).
//! - `Now`: the playhead and the selection.
//! - `Transcript`: what is said on the timeline (`speech::spoken`, so cut words are gone), one line per speaker turn,
//!   or per sentence when a turn runs over `MAX_LINE_WORDS` words (a sentence that long is split evenly):
//!   `[12.4-18.9] A: text`. Fillers keep their text ("um"); pause words are never text, but a gap of `SHOWN_PAUSE_US`
//!   or more between words reads `[pause 1.4 s]` where it falls, a gap between lines at the end of the first.
//! - `Scenes`: the scene log's items, `[40.5-45.0] gesture (male astronaut): raises an arm`, mapped from their file's
//!   time through every clip of that file on the main track. An item cut out entirely is left out; a partly cut one
//!   keeps what remains (a sliver under `MIN_SCENE_PIECE_US` of a longer item is dropped).
//!
//! Every time is TIMELINE seconds with one decimal, as the model reads and writes them. The same input always gives the
//! same bytes, so the provider's prompt cache can reuse the prefix: the maps given are only looked up, never walked.
//!
//! The speaker turns and scene items are neutral inputs keyed by asset id (what the analysis stored, in that file's
//! time): no provider's type reaches this crate.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use klipolo_model::Project;
use klipolo_model::captions::styles::default_style;
use klipolo_model::ids::Id;
use klipolo_model::project::{AssetKind, FormatKind};
use klipolo_model::query::{clip_at, clip_starts, video_duration};
use klipolo_model::speech::{
    Speech, Spoken, Transcript, WordKind, joins_unspaced, spoken, voice_runs,
};
use klipolo_model::time::{RangeUs, Us, scaled};
use klipolo_model::timeline::{Clip, ClipSource};
use serde::{Deserialize, Serialize};

/// The most the context may take of the model's input, in tokens (`estimate_tokens`).
pub const MAX_CONTEXT_TOKENS: usize = 12_000;
/// The estimate's characters per token.
pub const CHARS_PER_TOKEN: usize = 4;
/// A transcript line's most words.
pub const MAX_LINE_WORDS: usize = 30;
/// A gap between spoken words this long or longer shows as `[pause 1.4 s]`.
pub const SHOWN_PAUSE_US: Us = 1_000_000;
/// What is left of a partly cut scene item shorter than this (and than the item) is left out.
pub const MIN_SCENE_PIECE_US: Us = 100_000;

/// Where the person is: the playhead and the selection, in timeline time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub playhead_us: Us,
    pub selection: Option<RangeUs>,
}

/// One item of a file's scene log, in that file's time (its picture's): what happens, who, and any text on screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneEntry {
    pub start_us: Us,
    pub end_us: Us,
    /// "answer", "gesture", "b_roll", "scene_cut" ...
    pub kind: String,
    /// Who is on screen or acting ("male astronaut"); empty for nobody in particular.
    pub who: String,
    pub text: String,
    #[serde(default)]
    pub on_screen_text: Option<String>,
}

/// A file's scene log: its items, sorted by start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLog {
    pub items: Vec<SceneEntry>,
}

/// One speaker's turn in a file, in its sound time (its transcript's). A file's turns are sorted by start; a word
/// belongs to the last turn starting at or before its middle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerTurn {
    pub start_us: Us,
    pub end_us: Us,
    /// The speaker's label ("A"); None when the turn has none.
    #[serde(default)]
    pub speaker: Option<String>,
}

/// The short names the context gives the main track's clips, `c1` for the first: tool calls name clips by them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Refs {
    clips: Vec<Id>,
}

impl Refs {
    /// The refs of `project`'s main track, in order.
    pub fn of(project: &Project) -> Self {
        Self {
            clips: project.timeline.main.iter().map(|c| c.id.clone()).collect(),
        }
    }

    /// The clip id `short` names (`c2`), if any. Only the exact form: no leading zero, no spaces, lowercase.
    pub fn clip(&self, short: &str) -> Option<&str> {
        let n = short.strip_prefix('c')?;
        if n.is_empty() || n.starts_with('0') || !n.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let i: usize = n.parse().ok()?;
        self.clips.get(i - 1).map(String::as_str)
    }

    /// The ref of the clip with id `clip_id`.
    pub fn ref_of(&self, clip_id: &str) -> Option<String> {
        self.clips.iter().position(|c| c == clip_id).map(short_ref)
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

/// What the model reads about the project, and the clip refs it names clips by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatContext {
    pub text: String,
    pub refs: Refs,
}

/// About how many tokens `text` takes: `CHARS_PER_TOKEN` characters each, rounded up.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// The context for `project` with its `speech`, each file's speaker `turns` and `scenes` (by asset id), and the
/// person's `view`.
pub fn build_context(
    project: &Project,
    speech: &Speech,
    turns: &HashMap<String, Vec<SpeakerTurn>>,
    scenes: &HashMap<String, SceneLog>,
    view: &View,
) -> ChatContext {
    let starts = clip_starts(&project.timeline.main);
    let spoken = spoken(project, speech);
    let said = said(project, speech, turns, &spoken);
    let lines = lines(&said);
    let pieces = scene_pieces(project, &starts, scenes);

    let mut text = String::new();
    project_section(&mut text, project, &starts);
    text.push('\n');
    now_section(&mut text, project, view);
    text.push('\n');
    text.push_str("## Transcript\n");
    for line in &lines {
        write_line(&mut text, &said, line);
    }
    let untranscribed = clips_where(project, |clip| {
        voice_runs(project, clip)
            .iter()
            .any(|run| !speech.transcripts.contains_key(&run.asset))
    });
    if !untranscribed.is_empty() {
        let _ = writeln!(text, "Not transcribed yet: {untranscribed}.");
    } else if lines.is_empty() {
        text.push_str("No speech.\n");
    }
    text.push('\n');
    text.push_str("## Scenes\n");
    for piece in &pieces {
        write_scene(&mut text, piece);
    }
    let unlogged = clips_where(project, |clip| match &clip.source {
        ClipSource::Asset { id } => {
            project
                .asset(id)
                .is_some_and(|a| a.kind == AssetKind::Video)
                && !scenes.contains_key(id)
        }
        ClipSource::Recording { .. } => false,
    });
    if !unlogged.is_empty() {
        let _ = writeln!(text, "No scene log yet: {unlogged}.");
    } else if pieces.is_empty() {
        text.push_str("No scene items.\n");
    }
    ChatContext {
        text,
        refs: Refs::of(project),
    }
}

/// `c1` for the first clip.
fn short_ref(index: usize) -> String {
    format!("c{}", index + 1)
}

/// Timeline seconds with one decimal, rounded to the nearest tenth ("12.4").
fn secs(us: Us) -> String {
    let tenths = us.max(0).saturating_add(50_000) / 100_000;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// `text` on one line: white space and control characters become single spaces, none at either end.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for part in text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|p| !p.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

/// The refs of the main clips `pick` chooses, comma separated.
fn clips_where(project: &Project, pick: impl Fn(&Clip) -> bool) -> String {
    let refs: Vec<String> = (project.timeline.main.iter().enumerate())
        .filter(|(_, clip)| pick(clip))
        .map(|(i, _)| short_ref(i))
        .collect();
    refs.join(", ")
}

fn format_name(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "landscape 16:9",
        FormatKind::Portrait => "portrait 9:16",
        FormatKind::Square => "square 1:1",
    }
}

/// A clip speed as people write it: "2", "1.5", "0.25".
fn speed(speed: f64) -> String {
    let fixed = format!("{speed:.2}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn project_section(out: &mut String, p: &Project, starts: &[Us]) {
    let primary = p.settings.primary_format;
    let mut formats = vec![format!("{} (primary)", format_name(primary))];
    formats.extend(
        (p.settings.formats.iter())
            .filter(|&&f| f != primary)
            .map(|&f| format_name(f).to_owned()),
    );
    let _ = writeln!(out, "## Project\nName: {}", one_line(&p.name));
    let _ = writeln!(out, "Formats: {}", formats.join(", "));
    let _ = writeln!(out, "Duration: {} s", secs(video_duration(&p.timeline)));
    if p.captions.enabled {
        let id = (p.captions.style.as_ref())
            .map_or_else(|| default_style(&p.settings.formats), |s| s.as_str());
        let style = id.strip_prefix("klipolo.builtin/caption.").unwrap_or(id);
        let _ = writeln!(out, "Captions: on, style {}", one_line(style));
    } else {
        out.push_str("Captions: off\n");
    }
    if p.timeline.main.is_empty() {
        out.push_str("Clips: none\n");
        return;
    }
    out.push_str("Clips on the main track:\n");
    for (i, (clip, &start)) in p.timeline.main.iter().zip(starts).enumerate() {
        let name = match &clip.source {
            ClipSource::Asset { id } => p
                .asset(id)
                .map_or("missing file".to_owned(), |a| one_line(&a.name)),
            ClipSource::Recording { .. } => "recording".to_owned(),
        };
        let end = start.saturating_add(clip.duration_us());
        let _ = write!(out, "{} {}-{} {name}", short_ref(i), secs(start), secs(end));
        let mut notes = Vec::new();
        if let ClipSource::Asset { id } = &clip.source
            && let Some(a) = p.asset(id)
            && a.kind != AssetKind::Image
            && (clip.source_in_us > 0 || clip.source_out_us < a.duration_us)
        {
            notes.push(format!(
                "file {}-{} of {}",
                secs(clip.source_in_us),
                secs(clip.source_out_us),
                secs(a.duration_us)
            ));
        }
        if clip.speed != 1.0 {
            notes.push(format!("speed {}x", speed(clip.speed)));
        }
        if clip.muted {
            notes.push("muted".to_owned());
        }
        if !notes.is_empty() {
            let _ = write!(out, " ({})", notes.join(", "));
        }
        out.push('\n');
    }
}

fn now_section(out: &mut String, p: &Project, view: &View) {
    let at = view.playhead_us.max(0);
    match clip_at(&p.timeline.main, at) {
        Some((i, _)) => {
            let _ = writeln!(out, "## Now\nPlayhead: {} in {}", secs(at), short_ref(i));
        }
        None => {
            let _ = writeln!(out, "## Now\nPlayhead: {} (end)", secs(at));
        }
    }
    match view.selection {
        Some(r) if !r.is_empty() => {
            let _ = writeln!(out, "Selection: {}-{}", secs(r.start), secs(r.end));
        }
        _ => out.push_str("Selection: none\n"),
    }
}

/// One spoken word on the timeline, as the transcript shows it: its times, its text (empty for a later piece of a
/// fix) and its speaker turn.
struct Said<'a> {
    start_us: Us,
    end_us: Us,
    text: &'a str,
    speaker: Option<&'a str>,
    /// A new turn starts here: another file, another turn, or a word that comes again (a copy, a reorder).
    turn_starts: bool,
}

/// A span's transcript and speaker turns, looked up once per span rather than once per word.
#[derive(Clone, Copy)]
struct SpanFile<'a> {
    span: u32,
    transcript: Option<&'a Transcript>,
    turns: Option<&'a [SpeakerTurn]>,
}

/// The spoken words in timeline order, pause words left out.
fn said<'a>(
    p: &'a Project,
    speech: &'a Speech,
    turns: &'a HashMap<String, Vec<SpeakerTurn>>,
    spoken: &'a Spoken,
) -> Vec<Said<'a>> {
    let mut out: Vec<Said<'a>> = Vec::with_capacity(spoken.words.len());
    let mut file: Option<SpanFile<'a>> = None;
    let mut last: Option<(&str, Option<usize>, u32)> = None;
    for (w, text) in spoken.shown(p, speech) {
        if w.kind == WordKind::Pause {
            continue;
        }
        let asset = spoken.asset(w);
        let SpanFile {
            transcript,
            turns: file_turns,
            ..
        } = match file {
            Some(f) if f.span == w.span => f,
            _ => *file.insert(SpanFile {
                span: w.span,
                transcript: speech.transcripts.get(asset).map(|t| &**t),
                turns: turns.get(asset).map(Vec::as_slice),
            }),
        };
        let turn = (transcript.and_then(|t| t.words.get(w.word as usize)))
            .zip(file_turns)
            .and_then(|(word, turns)| {
                let mid = word.start_us + (word.end_us - word.start_us) / 2;
                turns.partition_point(|t| t.start_us <= mid).checked_sub(1)
            });
        let turn_starts = last.is_none_or(|(a, t, i)| a != asset || t != turn || w.word <= i);
        last = Some((asset, turn, w.word));
        out.push(Said {
            start_us: w.start_us,
            end_us: w.end_us,
            text,
            speaker: turn.and_then(|i| file_turns?[i].speaker.as_deref()),
            turn_starts,
        });
    }
    out
}

/// Whether a word ends a sentence ("safe.", "trips?", "great!\"").
fn ends_sentence(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’', ')', ']'])
        .ends_with(['.', '?', '!', '…', '。', '？', '！'])
}

/// The transcript's lines, as ranges of `said`: a turn of up to `MAX_LINE_WORDS` words is one line; a longer one
/// breaks at its sentence ends, and a sentence over `MAX_LINE_WORDS` into even parts.
fn lines(said: &[Said]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    for i in 1..=said.len() {
        if i < said.len() && !said[i].turn_starts {
            continue;
        }
        if i - from <= MAX_LINE_WORDS {
            out.push(from..i);
        } else {
            let mut sentence = from;
            for (j, w) in said[from..i].iter().enumerate() {
                let end = from + j + 1;
                if end == i || ends_sentence(w.text) {
                    even_parts(&mut out, sentence..end);
                    sentence = end;
                }
            }
        }
        from = i;
    }
    out
}

/// `words` in as few parts of at most `MAX_LINE_WORDS` as it takes, as even as can be.
fn even_parts(out: &mut Vec<Range<usize>>, words: Range<usize>) {
    let n = words.len();
    let parts = n.div_ceil(MAX_LINE_WORDS).max(1);
    let mut at = words.start;
    for k in 0..parts {
        let len = n / parts + usize::from(k < n % parts);
        out.push(at..at + len);
        at += len;
    }
}

/// `[pause 1.4 s]` for a gap of `SHOWN_PAUSE_US` or more.
fn pause_mark(gap: Us) -> Option<String> {
    (gap >= SHOWN_PAUSE_US).then(|| format!("[pause {} s]", secs(gap)))
}

/// One transcript line: `[a-b] A: text`, pauses marked. A line with no text to show (only later pieces of fixes)
/// writes nothing.
fn write_line(out: &mut String, said: &[Said], line: &Range<usize>) {
    let words = &said[line.clone()];
    if words.iter().all(|w| w.text.is_empty()) {
        return;
    }
    let (first, last) = (&words[0], &words[words.len() - 1]);
    let _ = write!(out, "[{}-{}] ", secs(first.start_us), secs(last.end_us));
    if let Some(speaker) = first.speaker.map(one_line).filter(|s| !s.is_empty()) {
        let _ = write!(out, "{speaker}: ");
    }
    // The last character written, to join words of scripts written without spaces (`joins_unspaced`).
    let mut before: Option<char> = None;
    for (k, w) in words.iter().enumerate() {
        if k > 0
            && let Some(mark) = pause_mark(w.start_us - words[k - 1].end_us)
        {
            if before.is_some() {
                out.push(' ');
            }
            out.push_str(&mark);
            before = Some(']');
        }
        if w.text.is_empty() {
            continue;
        }
        let text = if w
            .text
            .contains(|c: char| c.is_whitespace() || c.is_control())
        {
            one_line(w.text).into()
        } else {
            std::borrow::Cow::Borrowed(w.text)
        };
        let Some(start) = text.chars().next() else {
            continue;
        };
        if let Some(prev) = before
            && !(joins_unspaced(prev) && joins_unspaced(start))
        {
            out.push(' ');
        }
        out.push_str(&text);
        before = text.chars().last();
    }
    if let Some(next) = said.get(line.end)
        && let Some(mark) = pause_mark(next.start_us - last.end_us)
    {
        out.push(' ');
        out.push_str(&mark);
    }
    out.push('\n');
}

/// A scene item where it plays on the timeline.
struct ScenePiece<'a> {
    start_us: Us,
    end_us: Us,
    item: &'a SceneEntry,
}

/// Every scene item's pieces on the timeline, in timeline order: each clip of its file on the main track plays the
/// part of it inside the clip. Pieces that meet both in the file and on the timeline (a clip split in two) are one.
fn scene_pieces<'a>(
    p: &Project,
    starts: &[Us],
    scenes: &'a HashMap<String, SceneLog>,
) -> Vec<ScenePiece<'a>> {
    let mut out: Vec<ScenePiece<'a>> = Vec::new();
    // Each (file, item)'s latest piece: its index in `out` and where in the file it ends.
    let mut latest: HashMap<(&str, usize), (usize, Us)> = HashMap::new();
    for (clip, &clip_start) in p.timeline.main.iter().zip(starts) {
        let ClipSource::Asset { id } = &clip.source else {
            continue;
        };
        let Some(log) = scenes.get(id) else {
            continue;
        };
        let (source_in, source_out) = (clip.source_in_us, clip.source_out_us);
        let at = |source: Us| clip_start.saturating_add(scaled(source - source_in, clip.speed));
        for (k, item) in log.items.iter().enumerate() {
            let (start, end) = (item.start_us, item.end_us.max(item.start_us));
            let (from, to) = (start.max(source_in), end.min(source_out));
            let moment_inside = start == end && (source_in..source_out).contains(&start);
            if from >= to && !moment_inside {
                continue;
            }
            let (from_t, to_t) = (at(from), at(to));
            if let Some(&(j, file_end)) = latest.get(&(id.as_str(), k))
                && file_end == from
                && out[j].end_us == from_t
            {
                out[j].end_us = to_t;
                latest.insert((id.as_str(), k), (j, to));
                continue;
            }
            latest.insert((id.as_str(), k), (out.len(), to));
            out.push(ScenePiece {
                start_us: from_t,
                end_us: to_t,
                item,
            });
        }
    }
    out.retain(|s| {
        let whole = s.item.end_us.saturating_sub(s.item.start_us).max(0);
        s.end_us - s.start_us >= MIN_SCENE_PIECE_US.min(whole)
    });
    // Stable: pieces that start and end together keep the clips' and the log's order.
    out.sort_by_key(|s| (s.start_us, s.end_us));
    out
}

/// One scene line: `[a-b] kind (who): text`, and the text on screen if any.
fn write_scene(out: &mut String, piece: &ScenePiece) {
    let item = piece.item;
    let kind = one_line(&item.kind);
    let kind = if kind.is_empty() {
        "moment".to_owned()
    } else {
        kind
    };
    let _ = write!(
        out,
        "[{}-{}] {kind}",
        secs(piece.start_us),
        secs(piece.end_us)
    );
    let who = one_line(&item.who);
    if !who.is_empty() {
        let _ = write!(out, " ({who})");
    }
    let text = one_line(&item.text);
    if !text.is_empty() {
        let _ = write!(out, ": {text}");
    }
    if let Some(shown) = (item.on_screen_text.as_deref())
        .map(one_line)
        .filter(|t| !t.is_empty())
    {
        let _ = write!(out, " Text on screen: \"{shown}\"");
    }
    out.push('\n');
}
