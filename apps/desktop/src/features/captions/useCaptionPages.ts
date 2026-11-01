import { useEffect, useState } from 'react';
import type { CaptionPagesQuery } from '../../bindings/CaptionPagesQuery';
import type { CaptionPagesView } from '../../bindings/CaptionPagesView';
import type { FormatKind } from '../../bindings/FormatKind';
import type { PageView } from '../../bindings/PageView';
import { captions } from '../../bridge/captions';
import { useProject } from '../../state/project';
import { startSpeechFeed, useSpeech } from '../../state/speech';

/** The latest answers by what they were asked with: the lane and the canvas asking the same window make one call. */
const recent = new Map<string, Promise<CaptionPagesView | null>>();
const KEEP = 4;

function ask(key: string): Promise<CaptionPagesView | null> {
  let answer = recent.get(key);
  if (!answer) {
    const [projectId, , , , format, query] = JSON.parse(key) as [
      string,
      number,
      number,
      number,
      FormatKind,
      CaptionPagesQuery,
    ];
    answer = captions.pages(projectId, format, query).catch(() => null);
    recent.set(key, answer);
    for (const old of recent.keys()) {
      if (recent.size <= KEEP) break;
      recent.delete(old);
    }
  }
  return answer;
}

/** The most pages one answer holds (`captions_pages`' `MAX_PAGES`). */
const MOST = 200;

/**
 * The Captions view's complete answers, by what they were asked at: kept apart from `recent`, so the lane's and the
 * canvas's windows never push out the chunks of a long list (past `KEEP` of them, 800 pages).
 */
const whole = new Map<string, Promise<CaptionPagesView | null>>();
const KEEP_WHOLE = 2;

/** Tries at asking every page before giving up on chunks that keep disagreeing. */
const TRIES = 3;

/**
 * Every page of `projectId`'s captions in `format` as one answer, `MOST` at a time. Its chunks must be one answer: the
 * same revision and the same count. When an edit lands in Rust between two of them, every chunk is asked again.
 */
async function askAll(projectId: string, format: FormatKind): Promise<CaptionPagesView | null> {
  const chunk = (from: number) =>
    captions.pages(projectId, format, { type: 'index', from, count: MOST }).catch(() => null);
  for (let attempt = 0; attempt < TRIES; attempt++) {
    const first = await chunk(0);
    if (!first || first.total <= first.pages.length) return first;
    const rest = [];
    for (let from = first.pages.length; from < first.total; from += MOST) rest.push(chunk(from));
    const more = await Promise.all(rest);
    if (more.every((v) => v !== null && v.revision === first.revision && v.total === first.total))
      return { ...first, capped: false, pages: [...first.pages, ...more.flatMap((v) => v?.pages ?? [])] };
  }
  return null;
}

/** `askAll`, one call for the same key; a failed answer is forgotten, so the next view asks again. */
function askWhole(key: string, projectId: string, format: FormatKind): Promise<CaptionPagesView | null> {
  let answer = whole.get(key);
  if (!answer) {
    answer = askAll(projectId, format);
    whole.set(key, answer);
    void answer.then((view) => {
      if (view === null && whole.get(key) === answer) whole.delete(key);
    });
    for (const old of whole.keys()) {
      if (whole.size <= KEEP_WHOLE) break;
      whole.delete(old);
    }
  }
  return answer;
}

/**
 * `next`, with each page that reads exactly as `prev`'s page of the same index kept as `prev`'s object: an edit to one
 * word draws its own line again, not every line.
 */
function reusePages(prev: CaptionPagesView | null, next: CaptionPagesView | null): CaptionPagesView | null {
  if (!prev || !next) return next;
  const old = new Map(prev.pages.map((p) => [p.index, p]));
  let kept = 0;
  const pages = next.pages.map((p) => {
    const was = old.get(p.index);
    if (was && JSON.stringify(was) === JSON.stringify(p)) {
      kept += 1;
      return was;
    }
    return p;
  });
  return kept === 0 ? next : { ...next, pages };
}

/**
 * A window of the open project's caption pages in `format`, or null while captions are off or nothing
 * was asked. It is asked again when the revision, the opening or what was said changes (`pagesEpoch`), or the window
 * moves; the last answer stays shown until the next one lands, so nothing flickers. Never per frame.
 */
export function useCaptionPages(
  format: FormatKind,
  query: CaptionPagesQuery | null,
): CaptionPagesView | null {
  return useCaptionAnswer(format, query).view;
}

/**
 * `useCaptionPages`, and whether the answer shown is the one for what is asked now (`current`): false while a new
 * revision's or window's answer is on its way and the last one still shows. `fresh`: it was asked at the project's
 * revision now, whatever window (so a selection can wait out an edit's round trip). `of`: the revision (with the
 * project, its opening and what was said) it was asked at, and `base` the one now: the words a selection holds are
 * behind only when theirs is older.
 */
export function useCaptionAnswer(
  format: FormatKind,
  query: CaptionPagesQuery | null,
): { view: CaptionPagesView | null; current: boolean; fresh: boolean; of: string | null; base: string } {
  const projectId = useProject((s) => s.snapshot?.project.id ?? null);
  const opening = useProject((s) => s.snapshot?.opening ?? 0);
  const revision = useProject((s) => s.snapshot?.history.revision ?? 0);
  const enabled = useProject((s) => s.snapshot?.project.captions.enabled ?? false);
  const epoch = useSpeech((s) => s.pagesEpoch);
  useEffect(() => startSpeechFeed(), []);
  const base = JSON.stringify([projectId, opening, revision, epoch]);
  const key =
    enabled && projectId !== null && query !== null
      ? JSON.stringify([projectId, opening, revision, epoch, format, query])
      : null;
  const [shown, setShown] = useState<{ key: string; base: string; view: CaptionPagesView | null } | null>(
    null,
  );
  useEffect(() => {
    if (key === null) return;
    let live = true;
    void ask(key).then((view) => {
      if (live) setShown({ key, base, view });
    });
    return () => {
      live = false;
    };
  }, [key, base]);
  if (key === null) return { view: null, current: false, fresh: false, of: null, base };
  return {
    view: shown?.view ?? null,
    current: shown?.key === key,
    fresh: shown?.base === base,
    of: shown?.base ?? null,
    base,
  };
}

/**
 * Every page of the open project's captions in `format`, as one answer (`first` 0), asked `MOST` at a time
 * (`askAll`): the Captions view lists them all, its lines wrapping to their words. Asked again as `useCaptionAnswer`
 * is; the last answer stays shown until the next one is complete, and a page that reads the same stays the same
 * object (`reusePages`). `fresh`, `of` and `base` as `useCaptionAnswer`'s.
 */
export function useAllCaptionPages(format: FormatKind): {
  view: CaptionPagesView | null;
  fresh: boolean;
  of: string | null;
  base: string;
} {
  const projectId = useProject((s) => s.snapshot?.project.id ?? null);
  const opening = useProject((s) => s.snapshot?.opening ?? 0);
  const revision = useProject((s) => s.snapshot?.history.revision ?? 0);
  const enabled = useProject((s) => s.snapshot?.project.captions.enabled ?? false);
  const epoch = useSpeech((s) => s.pagesEpoch);
  useEffect(() => startSpeechFeed(), []);
  const base = JSON.stringify([projectId, opening, revision, epoch]);
  const key = enabled && projectId !== null ? JSON.stringify([base, format]) : null;
  const [shown, setShown] = useState<{ key: string; base: string; view: CaptionPagesView | null } | null>(
    null,
  );
  useEffect(() => {
    if (key === null || projectId === null) return;
    let live = true;
    void askWhole(key, projectId, format).then((view) => {
      if (live) setShown((was) => ({ key, base, view: reusePages(was?.view ?? null, view) }));
    });
    return () => {
      live = false;
    };
  }, [key, base, projectId, format]);
  if (key === null) return { view: null, fresh: false, of: null, base };
  return { view: shown?.view ?? null, fresh: shown?.base === base, of: shown?.base ?? null, base };
}

/** The page of `view` showing at `us` (its pages are in order and never overlap): a binary search. */
export function pageAt(view: CaptionPagesView | null, us: number): PageView | null {
  const pages = view?.pages ?? [];
  let lo = 0;
  let hi = pages.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (pages[mid].startUs <= us) lo = mid + 1;
    else hi = mid;
  }
  const page = pages[lo - 1];
  return page && us < page.endUs ? page : null;
}

/** Forget the answers kept (tests). */
export function forgetCaptionPages() {
  recent.clear();
  whole.clear();
}
