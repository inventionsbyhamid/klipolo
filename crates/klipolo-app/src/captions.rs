//! The caption pages the Captions panel shows: a window of the open project's pages in one format at a time
//! (`answer`), from the model's own `captions`, so they are the pages the engine draws and an export freezes; and every
//! place a word shows (`find`), for "Fix all". One build is kept while what pages are made from is unchanged. No Tauri
//! types: the app's commands (`commands::captions`) read the engine's feed and call in here.
//!
//! Cost: a build is O(words × formats), made only when a view asks after the speech or what pages are made from
//! changed. An answer is two binary searches plus at most `MAX_PAGES` pages copied, each word's said text (and a
//! correction's whole range) found by binary search in the project's fixes and its transcript. `find` walks one
//! format's words once.

mod view;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_engine::plan::captions::pages_may_differ;
use klipolo_engine::session::ProjectFeed;
use klipolo_model::captions::{Captions, Page, Pages, captions};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, WordKind};
use klipolo_model::time::Us;

pub use view::{CaptionPagesView, PageView, PagesQuery, WordView};

/// The most pages one answer holds: a long list is asked a window at a time.
pub const MAX_PAGES: usize = 200;

/// The open project's pages, one build kept (the app's state).
#[derive(Default)]
pub struct CaptionsCache {
    kept: Mutex<Option<Kept>>,
    builds: AtomicUsize,
}

/// A build and what it was made from. The speech is held, so its address can't be reused by a newer one.
struct Kept {
    project_id: String,
    opened: u64,
    revision: u64,
    project: Arc<Project>,
    speech: Arc<Speech>,
    captions: Arc<Captions>,
}

impl CaptionsCache {
    /// How many builds were made (a revision that pages alike reuses the one kept).
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::Relaxed)
    }

    /// `feed`'s pages: the kept build when it was made from the same project, opening and speech, and the revision is
    /// the same or one whose pages can't differ (a colour, a place, a title: the engine's own `pages_may_differ`,
    /// O(clips)); else a new one in its place. The lock is held while it builds, so two views asking at once build once.
    fn captions(&self, feed: &ProjectFeed) -> Arc<Captions> {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let same = kept.as_mut().is_some_and(|k| {
            let ours = k.project_id == feed.project_id
                && k.opened == feed.opened
                && Arc::ptr_eq(&k.speech, &feed.speech);
            let pages = k.revision == feed.revision || !pages_may_differ(&k.project, &feed.project);
            if ours && pages {
                (k.revision, k.project) = (feed.revision, feed.project.clone());
            }
            ours && pages
        });
        if !same {
            self.builds.fetch_add(1, Ordering::Relaxed);
            *kept = Some(Kept {
                project_id: feed.project_id.clone(),
                opened: feed.opened,
                revision: feed.revision,
                project: feed.project.clone(),
                speech: feed.speech.clone(),
                captions: Arc::new(captions(&feed.project, &feed.speech)),
            });
        }
        kept.as_ref()
            .map_or_else(Arc::default, |k| k.captions.clone())
    }

    /// No project is open: the build goes.
    fn forget(&self) {
        *self.kept.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// The window `query` asks of `pages` (in order, never overlapping): `[from, to)`, at most `MAX_PAGES` long, and
/// whether it was cut there.
fn window(pages: &[Page], query: PagesQuery) -> (usize, usize, bool) {
    let (from, to) = match query {
        PagesQuery::Range { from_us, to_us } => {
            let from = pages.partition_point(|p| p.end_us <= from_us);
            (
                from,
                pages.partition_point(|p| p.start_us < to_us).max(from),
            )
        }
        PagesQuery::Index { from, count } => {
            let from = (from as usize).min(pages.len());
            (from, from.saturating_add(count as usize).min(pages.len()))
        }
    };
    (from, to.min(from + MAX_PAGES), to > from + MAX_PAGES)
}

/// What a word reads as heard or fixed, before the style's case and marks.
struct Said<'a> {
    text: Option<&'a str>,
    /// The correction's whole range, when the word reads one.
    fix: Option<(Us, Us)>,
    /// The transcript heard a filler there.
    filler: bool,
    /// What the transcript heard over the correction's range.
    heard: Option<String>,
}

/// The transcript's words heard in `[from, to)` of its file, joined as said (pauses left out).
fn heard_in(transcript: &Transcript, from: Us, to: Us) -> String {
    let k = transcript.words.partition_point(|w| w.start_us < from);
    let mut out = String::new();
    for w in transcript.words[k..].iter().take_while(|w| w.start_us < to) {
        if w.kind == WordKind::Pause || w.text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w.text.trim());
    }
    out
}

/// What the words `asset` plays from `from` to `to` in its own time say: the fix over their middle (fixes are sorted by
/// file and start and never overlap), else the transcript word starting there. O(log fixes + log words).
fn said<'a>(p: &'a Project, speech: &'a Speech, asset: &str, from: Us, to: Us) -> Said<'a> {
    let mid = from + (to - from) / 2;
    let fixes = &p.word_fixes;
    let at = fixes.partition_point(|f| (f.asset_id.as_str(), f.start_us) <= (asset, mid));
    let transcript = speech.transcripts.get(asset);
    if let Some(f) = at
        .checked_sub(1)
        .map(|i| &fixes[i])
        .filter(|f| f.asset_id == asset && mid < f.end_us)
    {
        let heard = transcript
            .map(|t| heard_in(t, f.start_us, f.end_us))
            .filter(|h| !h.is_empty() && h != f.text.trim());
        return Said {
            text: Some(f.text.trim()),
            fix: Some((f.start_us, f.end_us)),
            filler: false,
            heard,
        };
    }
    let word = transcript.and_then(|t| {
        let k = t.words.partition_point(|w| w.start_us < from);
        t.words.get(k).filter(|w| w.start_us == from)
    });
    Said {
        text: word.map(|w| w.text.trim()),
        fix: None,
        filler: word.is_some_and(|w| w.kind == WordKind::Filler),
        heard: None,
    }
}

fn page_view(p: &Project, speech: &Speech, pages: &Pages, page: &Page) -> PageView {
    let words = (page.words.0..page.words.1)
        .filter_map(|i| {
            let w = pages.words.get(i as usize)?;
            let asset = pages.assets.get(w.asset as usize)?;
            let text = pages.word_text(page, i);
            let said = said(p, speech, asset, w.source_start_us, w.source_end_us);
            let (source_start_us, source_end_us) =
                said.fix.unwrap_or((w.source_start_us, w.source_end_us));
            // The shared text has a space before every word but the first and those their script joins (`Pages`).
            let joined = (w.text.0 as usize)
                .checked_sub(1)
                .and_then(|b| pages.text.as_bytes().get(b))
                .is_none_or(|&b| b != b' ');
            Some(WordView {
                text: text.to_owned(),
                said: said.text.filter(|s| *s != text).map(str::to_owned),
                joined,
                start_us: w.start_us,
                end_us: w.end_us,
                asset_id: asset.clone(),
                source_start_us,
                source_end_us,
                look: pages.look(w).clone(),
                keyword: pages.keyword(i),
                cut_before: w.cut_before.then_some(true),
                // The piece's middle, where its look is read, when the range sent is the whole correction's.
                look_us: said
                    .fix
                    .map(|_| w.source_start_us + (w.source_end_us - w.source_start_us) / 2),
                filler: said.filler.then_some(true),
                heard: said.heard,
            })
        })
        .collect();
    PageView {
        index: page.index,
        start_us: page.start_us,
        end_us: page.end_us,
        words,
    }
}

/// `query`'s window of `format`'s pages in `feed`, when it is project `project_id`'s; None when another project (or
/// none) is open.
pub fn answer(
    cache: &CaptionsCache,
    feed: Option<&ProjectFeed>,
    project_id: &str,
    format: FormatKind,
    query: PagesQuery,
) -> Option<CaptionPagesView> {
    let Some(feed) = feed else {
        cache.forget();
        return None;
    };
    if feed.project_id != project_id {
        return None;
    }
    let captions = cache.captions(feed);
    let all = captions.pages.get(&format);
    let list = all.map_or(&[][..], |p| &p.pages[..]);
    let (from, to, capped) = window(list, query);
    Some(CaptionPagesView {
        revision: feed.revision,
        format,
        total: list.len() as u32,
        first: from as u32,
        capped,
        pages: all.map_or_else(Vec::new, |pages| {
            list[from..to]
                .iter()
                .map(|page| page_view(&feed.project, &feed.speech, pages, page))
                .collect()
        }),
    })
}

/// A word as matched: lowercase, without the marks around it.
fn bare(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Every place `format`'s pages show `text` (matched whole, ignoring case and the marks around it, as said or fixed,
/// never in the style's capitals), in timeline order, hidden words left out: one per file word, so the copies of a
/// clip, which one fix covers, count once. What "Fix all" fixes: matched whatever their case, so the page gives each
/// place its own first letter's case ("The" starting a sentence stays capital). None when `project_id` isn't open;
/// empty for a text with no letter or digit.
pub fn find(
    cache: &CaptionsCache,
    feed: Option<&ProjectFeed>,
    project_id: &str,
    format: FormatKind,
    text: &str,
) -> Option<Vec<WordView>> {
    let feed = feed.filter(|f| f.project_id == project_id)?;
    let wanted = bare(text);
    if wanted.is_empty() {
        return Some(Vec::new());
    }
    let captions = cache.captions(feed);
    let Some(pages) = captions.pages.get(&format) else {
        return Some(Vec::new());
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for page in &pages.pages {
        let view = page_view(&feed.project, &feed.speech, pages, page);
        for w in view.words {
            if w.look.hidden == Some(true) || bare(w.said.as_deref().unwrap_or(&w.text)) != wanted {
                continue;
            }
            if seen.insert((w.asset_id.clone(), w.source_start_us, w.source_end_us)) {
                out.push(w);
            }
        }
    }
    Some(out)
}
