import { useLingui } from '@lingui/react/macro';
import { memo, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import type { FormatKind } from '../../bindings/FormatKind';
import type { PageView } from '../../bindings/PageView';
import type { WordView } from '../../bindings/WordView';
import { captions } from '../../bridge/captions';
import { cn } from '../../design/cn';
import { SpriteIcon } from '../../design/SpriteIcon';
import { formatPrecise } from '../../lib/time';
import { useStableCallback } from '../../lib/useStableCallback';
import { offerAgain, useAnalysedClips, useAnalysis, useTakes, whyNoWords } from '../../state/analysis';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { pageAt, useAllCaptionPages } from './useCaptionPages';
import { lookedNow, usePressQueue } from './usePressQueue';
import { fileWordKey, useWordSelection, wordKey, type Listed } from './useWordSelection';
import { WordColor } from './WordColor';
import { WordEditor } from './WordEditor';

/** The DOM id of a word's chip. */
export const chipId = (prefix: string, key: string) => `${prefix}-${key.replace(/[^\w-]/g, '_')}`;

const seek = (w: WordView) => usePlayback.getState().seek(w.startUs);

/** Why the open project has no caption words (`whyNoWords`), from the analysis of what its main-track clips play. */
function useWhyNoWords() {
  const analysed = useAnalysedClips();
  const clips = useMemo(() => analysed.map((a) => a.id), [analysed]);
  const takes = useTakes();
  return useAnalysis((s) => whyNoWords(s.states, clips, takes));
}

/** The word being fixed in place, as it was when the field opened, and the places it shows ("Fix all"). */
interface Edit {
  word: WordView;
  matches: WordView[] | null;
}

/** What the editor needs from the list, the same for every line (and the same object while the list is up). */
interface EditorProps {
  spell: Parameters<typeof WordEditor>[0]['spell'];
  typed: () => void;
  fixAll: Parameters<typeof WordEditor>[0]['fixAll'];
  hide: () => void;
  close: () => void;
  list: RefObject<HTMLDivElement | null>;
}

/**
 * One caption page as a line of the list: its time on the left, its word chips on the right. A filler
 * ("um", "uh") is dashed and italic, a hidden word struck through with an eye-off icon (a click shows it again), a fixed
 * one ringed, saying what it was fixed from. The word being fixed is a field in its chip's place, its fix bar under
 * the line. Drawn again only when something about this line changes.
 */
const Line = memo(function Line({
  page,
  row,
  now,
  idPrefix,
  selected,
  active,
  edit,
  busy,
  editor,
  onPress,
}: {
  page: PageView;
  row: number;
  now: boolean;
  idPrefix: string;
  /** The selected words' keys on this line (none: null). */
  selected: string | null;
  /** The active word's key, when it is on this line. */
  active: string | null;
  /** The word being fixed, when it is on this line, by its key now. */
  edit: (Edit & { key: string }) | null;
  /** The words are being updated: given only to the line being edited, so the others aren't drawn again for it. */
  busy: boolean;
  editor: EditorProps;
  onPress: (word: WordView) => void;
}) {
  const { t } = useLingui();
  const time = formatPrecise(page.startUs);
  const keys = selected ? new Set(selected.split('\n')) : null;
  return (
    <div
      id={`${idPrefix}-line-${row}`}
      role="group"
      aria-label={time}
      className={cn('line', now && 'is-now')}
    >
      <span className="line-when" aria-hidden>
        {time}
      </span>
      <div className="chips">
        {page.words.map((w) => {
          const key = wordKey(w);
          if (edit && edit.key === key)
            return (
              <WordEditor
                key={fileWordKey(edit.word)}
                word={edit.word}
                matches={edit.matches}
                busy={busy}
                {...editor}
              />
            );
          const hidden = w.look.hidden === true;
          const hint = hidden
            ? t({ id: 'captions.words.hiddenHint', message: 'Hidden. Click to show.' })
            : w.filler
              ? t({ id: 'captions.words.fillerHint', message: 'Filler' })
              : w.heard
                ? t({ id: 'captions.words.fixedHint', message: `Fixed from ${w.heard}` })
                : undefined;
          return (
            <span
              key={key}
              id={chipId(idPrefix, key)}
              role="option"
              aria-selected={keys?.has(key) ?? false}
              aria-description={hint}
              title={hint}
              onMouseDown={(e) => {
                // The list keeps the focus, so the keys that follow go on from this word.
                e.preventDefault();
                editor.list.current?.focus({ preventScroll: true });
              }}
              onClick={() => onPress(w)}
              className={cn(
                'word',
                w.filler && 'is-filler',
                hidden && 'is-hidden',
                !!w.heard && 'is-fixed',
                key === active && 'is-active',
              )}
            >
              {hidden && <SpriteIcon name="eye-off" />}
              {w.text}
            </span>
          );
        })}
      </div>
    </div>
  );
});

/** Whether `page` has a word reading `query` (as shown or as said), ignoring case. */
const hasWord = (page: PageView, query: string) =>
  page.words.some(
    (w) => w.text.toLowerCase().includes(query) || (w.said ?? '').toLowerCase().includes(query),
  );

/**
 * The caption words: a "Words" heading with Find a word, then one line per caption page of the format
 * previewed, every page asked 200 at a time. The list follows the playhead (the line now playing gets brighter chips
 * and an ink time) until the person scrolls it, and again once playback starts. A click on a word moves the playhead
 * to it and opens it for fixing (`WordEditor`); a click on a hidden word shows it again. The list is one listbox with
 * one tab stop: the arrow keys move the active word, Enter fixes it, H hides it (or shows it again), Escape lets go.
 * Find a word shows only the lines with it (or says none has it), and the keys move through those lines alone.
 * The word picked lasts through refetches (`useWordSelection`); a fix or a hide made while its words are being updated
 * waits for them (`usePressQueue`), and "Updating…" says so.
 */
export function CaptionWords({ format }: { format: FormatKind }) {
  const { t } = useLingui();
  const idPrefix = `words${useId().replace(/[^\w-]/g, '')}`;
  const listRef = useRef<HTMLDivElement>(null);
  const projectId = useProject((s) => s.snapshot?.project.id ?? null);
  const enabled = useProject((s) => s.snapshot?.project.captions.enabled ?? false);
  const { view, fresh, of } = useAllCaptionPages(format);
  const total = view?.total ?? 0;
  const noWords = useWhyNoWords();
  const { sel, picked, heldPick, listed, nav, words, selected, pending, find, pick, clear } = useWordSelection(
    format,
    view,
    of,
  );
  const { busy, note, press, spell, typed, fixAll } = usePressQueue({
    words,
    picked,
    heldPick,
    pending,
    fresh,
    find,
  });
  const [edit, setEdit] = useState<Edit | null>(null);
  const [query, setQuery] = useState('');
  /** A hidden word clicked: shown again once it is the word picked. */
  const [showing, setShowing] = useState<string | null>(null);

  // The word being fixed where the answer has it now (its key and page can change with an edit before it), found in
  // this render, so its field never leaves the page while the selection catches up.
  const editKey = edit && words.length === 1 ? wordKey(words[0]) : null;
  const editRow = editKey === null ? -1 : (listed.find((l) => l.key === editKey)?.row ?? -1);
  const q = query.trim().toLowerCase();
  const pages = view?.pages ?? [];
  /** The lines a search shows (every line without one): those with the word, and the one being fixed. */
  const shownRows = useMemo(
    () => (q ? new Set(pages.flatMap((page, i) => (hasWord(page, q) || i === editRow ? [i] : []))) : null),
    [pages, q, editRow],
  );

  // The keys move through the answer that holds the active word, and only through the lines a search shows.
  const all = nav?.list ?? listed;
  const list = shownRows ? all.filter((l) => shownRows.has(l.row)) : all;
  const at = sel && shownRows ? list.findIndex((l) => l.key === sel.focus) : -1;
  const focus = shownRows ? (at < 0 ? undefined : at) : sel && nav ? nav.at.get(sel.focus) : undefined;

  /** The counts asked so far: only the latest one's answer is taken, so an older one never overwrites it. */
  const counts = useRef(0);
  /**
   * Asks where else `w`, being fixed, shows. When that can't be asked, the word counts as showing here only: its fix
   * bar fixes just this one (never waits for a count that won't come).
   */
  const countPlaces = (w: WordView) => {
    if (!projectId) return;
    counts.current += 1;
    const mine = counts.current;
    void captions
      .find(projectId, format, w.said ?? w.text)
      .then(
        (matches) => matches ?? [w],
        () => [w],
      )
      .then((matches) => {
        if (mine === counts.current) setEdit((e) => (e?.word === w ? { ...e, matches } : e));
      });
  };
  /** Opens `w` for fixing, and asks where else it shows. */
  const open = (w: WordView) => {
    setEdit({ word: w, matches: null });
    countPlaces(w);
  };
  // The words updated (an edit, new speech) with a word open: where it shows is asked again, and counted as not known
  // until then, so Fix all never fixes places that moved.
  const placesOf = useRef(of);
  useEffect(() => {
    if (placesOf.current === of) return;
    placesOf.current = of;
    if (!edit) return;
    setEdit((e) => (e ? { ...e, matches: null } : e));
    countPlaces(edit.word);
  }, [of, edit]);
  const toggleHidden = () => {
    if (words.length === 0) return;
    const hidden = lookedNow(words).every((w) => w.look.hidden === true);
    press(hidden ? { hidden: undefined } : { hidden: true });
  };
  const onPress = useStableCallback((word: WordView) => {
    const l = listed.find((x) => x.word === word);
    if (!l) return;
    pick(l, false);
    following.current = true;
    if (word.look.hidden) {
      setEdit(null);
      setShowing(l.key);
      return;
    }
    seek(word);
    open(word);
  });
  useEffect(() => {
    if (showing === null || words.length !== 1 || wordKey(words[0]) !== showing) return;
    setShowing(null);
    press({ hidden: undefined });
  }, [showing, words, press]);

  /** Makes `l` the active word and brings it into view. */
  const move = (l: Listed | undefined) => {
    if (!l) return;
    pick(l, false);
    document.getElementById(chipId(idPrefix, l.key))?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  };
  /** The first word of the page `by` rows from the active word's. */
  const rowStep = (by: number) => {
    const row = (focus === undefined ? (list[0]?.row ?? 0) : list[focus].row) + by;
    const k = list.findIndex((l) => l.row >= row);
    return k < 0 ? list.length - 1 : k;
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    const here = focus ?? -1;
    const to: Partial<Record<string, () => number>> = {
      ArrowRight: () => Math.min(list.length - 1, here + 1),
      ArrowLeft: () => Math.max(0, here - 1),
      ArrowDown: () => rowStep(1),
      ArrowUp: () => rowStep(-1),
      Home: () => 0,
      End: () => list.length - 1,
    };
    const step = to[e.key];
    if (e.key === 'Escape' && sel) {
      e.preventDefault();
      setEdit(null);
      clear();
    } else if (e.key === 'Enter' && focus !== undefined) {
      e.preventDefault();
      const w = list[focus].word;
      seek(w);
      if (!w.look.hidden) open(w);
    } else if ((e.key === 'h' || e.key === 'H') && !e.metaKey && !e.ctrlKey && !e.altKey && sel) {
      e.preventDefault();
      // Not the editor's H: hiding the word, not anything on the timeline.
      e.stopPropagation();
      toggleHidden();
    } else if (step) {
      e.preventDefault();
      move(list[step()]);
    }
  };

  // The line now playing, found by binary search; the list draws again only when it changes.
  const nowRow = usePlayback((s) => {
    const page = pageAt(view, s.playheadUs);
    return page && view ? view.pages.indexOf(page) : -1;
  });
  /** The list follows the playhead until the person scrolls it, and again once playback starts or a word is clicked. */
  const following = useRef(true);
  useEffect(() => {
    const box = listRef.current?.closest('.cap-scroll');
    if (!box) return;
    const stop = () => (following.current = false);
    box.addEventListener('wheel', stop, { passive: true });
    box.addEventListener('touchmove', stop, { passive: true });
    const off = usePlayback.subscribe((s, was) => {
      if (s.playing && !was.playing) following.current = true;
    });
    return () => {
      box.removeEventListener('wheel', stop);
      box.removeEventListener('touchmove', stop);
      off();
    };
  }, []);
  useEffect(() => {
    if (nowRow < 0 || !following.current || edit) return;
    document.getElementById(`${idPrefix}-line-${nowRow}`)?.scrollIntoView?.({ block: 'nearest' });
  }, [nowRow, idPrefix, edit]);

  // The selected word went (another project, a refetch that lost it): so does its field.
  useEffect(() => {
    if (!sel && edit) setEdit(null);
  }, [sel, edit]);

  const close = useStableCallback(() => setEdit(null));
  const hide = useStableCallback(() => void press({ hidden: true }));
  const editor = useMemo<EditorProps>(
    () => ({ spell, typed, fixAll, hide, close, list: listRef }),
    [spell, typed, fixAll, hide, close],
  );

  // Each line's share of the selection, as one string, so a line draws again only when its own part changes.
  const byRow = useMemo(() => {
    const out = new Map<number, string[]>();
    for (const l of listed) if (selected.has(l.key)) (out.get(l.row) ?? out.set(l.row, []).get(l.row)!).push(l.key);
    return out;
  }, [listed, selected]);
  const active = sel ? sel.focus : null;
  /** The active word's chip is on screen: a search may have filtered its line out. */
  const activeShown = active !== null && (!shownRows || (sel !== null && shownRows.has(sel.focusRow)));

  let hint: string | null = null;
  if (!enabled)
    hint = t({
      id: 'captions.words.off',
      message: 'Pick a style to turn captions on. Then you can fix any word here.',
    });
  else if (view === null) hint = t({ id: 'captions.words.loading', message: 'Getting the words…' });
  else if (total === 0 && noWords === 'noSpeechOne')
    hint = t({ id: 'captions.words.noSpeech', message: 'No speech found in this clip.' });
  else if (total === 0 && noWords === 'noSpeechMany')
    hint = t({ id: 'captions.words.noSpeechAll', message: 'No speech found in these clips.' });
  else if (total === 0)
    hint = t({
      id: 'captions.words.none',
      message: 'No caption words yet. They show here once Klipolo has heard the clips.',
    });

  return (
    <section aria-labelledby={`${idPrefix}-title`} className="words">
      <div className="words-head">
        <h3 id={`${idPrefix}-title`}>{t({ id: 'captions.words.title', message: 'Words' })}</h3>
        {words.length > 0 && total > 0 && <WordColor words={words} busy={busy} press={press} />}
        <label className="search">
          <SpriteIcon name="search" />
          <input
            type="search"
            value={query}
            placeholder={t({ id: 'captions.words.find', message: 'Find a word' })}
            aria-label={t({ id: 'captions.words.find', message: 'Find a word' })}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.stopPropagation()}
          />
        </label>
      </div>
      {hint && <p className="words-hint">{hint}</p>}
      {enabled && view !== null && total === 0 && noWords === 'notAnalysed' && (
        // The offer comes back in the analysis banner, with its price, even after "Not now".
        <button
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="btn btn-secondary btn-sm words-analyze"
          onClick={() => void offerAgain()}
        >
          {t({ id: 'captions.words.analyze', message: 'Analyze' })}
        </button>
      )}
      <p role="status" className="words-note">
        {busy
          ? t({ id: 'captions.words.updating', message: 'Updating…' })
          : shownRows && shownRows.size === 0 && total > 0
            ? t({ id: 'captions.words.noMatch', message: 'No word matches.' })
            : note}
      </p>
      <div
        ref={listRef}
        role={total > 0 ? 'listbox' : undefined}
        tabIndex={total > 0 ? 0 : undefined}
        aria-label={total > 0 ? t({ id: 'captions.words.list', message: 'Caption words' }) : undefined}
        // Said, not shown: the panel keeps its height for words.
        aria-description={
          total > 0
            ? t({
                id: 'captions.words.how',
                message: 'Click a word to fix it. Your changes stay when you switch styles.',
              })
            : undefined
        }
        aria-activedescendant={total > 0 && active && activeShown && !edit ? chipId(idPrefix, active) : undefined}
        // A list that is part of the panel, not a popup: Command Z and Shift Command Z still reach the editor from it.
        data-editor-keys
        onKeyDown={onKey}
        className="word-lines"
      >
        {pages.map((page, i) =>
          shownRows && !shownRows.has(i) ? null : (
            <Line
              key={page.index}
              page={page}
              row={i}
              now={i === nowRow}
              idPrefix={idPrefix}
              selected={byRow.get(i)?.join('\n') ?? null}
              active={active && sel?.focusRow === i ? active : null}
              edit={i === editRow && edit && editKey ? { ...edit, key: editKey } : null}
              busy={i === editRow && busy}
              editor={editor}
              onPress={onPress}
            />
          ),
        )}
      </div>
    </section>
  );
}
