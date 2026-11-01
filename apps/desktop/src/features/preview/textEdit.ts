import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { Item } from '../../bindings/Item';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import { engine } from '../../bridge/engine';
import type { LabelId } from '../../lib/labels';
import { itemEndUs, showItemAtUs } from '../../lib/timeline';
import { registerBurst } from '../../state/bursts';
import { afterCommits, beginCommit, keepAtClose } from '../../state/commitQueue';
import { capture, changedSince, type Built } from '../../state/editBase';
import { framesShown } from '../../state/framesShown';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';

/** The most characters a title holds (the sidebar field's limit). */
export const MAX_TEXT = 500;
/** How long the finished edit's look waits for the preview to draw the title again (it may draw nothing). */
const HANDOFF_CAP_MS = 200;

/** `text` capped at `MAX_TEXT`, never between an emoji's two halves (a lone half isn't text Rust takes). */
export function capText(text: string): string {
  if (text.length <= MAX_TEXT) return text;
  const last = text.charCodeAt(MAX_TEXT - 1);
  return text.slice(0, last >= 0xd800 && last <= 0xdbff ? MAX_TEXT - 1 : MAX_TEXT);
}

/** A title being typed on the canvas (and in the sidebar field: one draft). */
export interface EditSession {
  /** The layer's id, or the new box's. */
  id: string;
  /** A new box ("Add text"): not in the project until the edit finishes with words. */
  draft: Item | null;
  /** The new box attaches to the clip under it (inside the video). */
  attach: boolean;
  text: string;
  startText: string;
  /** Where the edit began: a change from elsewhere since then makes Rust refuse it, and the words are kept. */
  built: Built;
  composing: boolean;
}

interface TextEditStore {
  session: EditSession | null;
  /**
   * The edit that just finished, with its final words: the editor still draws its look (nothing to type in) until the
   * preview draws the layer again, so it never blinks out.
   */
  closing: EditSession | null;
  /** An existing layer: its words, the cursor at the end. A locked track's layer doesn't begin. */
  begin: (id: string) => void;
  /** A new box: "Your text" selected. */
  beginDraft: (item: Item, attach: boolean) => void;
  setText: (text: string) => void;
  setComposing: (on: boolean) => void;
  /**
   * Commits once (or nothing), then clears the session. Its commit waits its turn, kept at close: as the project closes
   * (or Klipolo quits) it goes in after a commit on its way, built on it, and the close waits for it.
   */
  finish: () => Promise<void>;
}

/** Text layer `id` in `project`, with whether its track is locked. */
export function findText(project: Project | undefined, id: string): { item: Item; locked: boolean } | null {
  for (const track of project?.timeline.tracks ?? [])
    for (const item of track.items)
      if (item.id === id && item.content.type === 'text') return { item, locked: track.locked };
  return null;
}

const currentProject = () => useProject.getState().snapshot?.project;

/**
 * Title `id`'s words as the project has them now, else `fallback`. A toolbar change that carries the title's whole
 * content builds its words from this at its turn (after the commit queue), so a press that finishes an edit on the
 * canvas never puts the old words back.
 */
export function latestText(id: string, fallback: string): string {
  const found = findText(currentProject(), id);
  return found ? (found.item.content as { text: string }).text : fallback;
}

/** The words to commit: capped, trailing spaces and lines dropped. */
const finalWords = (s: EditSession) => capText(s.text).replace(/\s+$/u, '');

interface Change {
  label: LabelId;
  ops: Op[];
}

/**
 * What finishing `s` with `words` commits, built from the project as it is when it goes in (after commits on their
 * way: its op carries the layer's whole content): an Add text, an Edit text, a Remove text for an emptied title, or
 * nothing. `undefined`: the layer is gone, so the words have nowhere to go.
 */
function changeOf(s: EditSession, words: string): Change | null | undefined {
  if (s.draft) {
    if (!words.trim()) return null;
    const item = { ...s.draft, content: { ...s.draft.content, text: words } } as Item;
    return { label: 'text.add', ops: [{ type: 'addItem', track: { type: 'auto' }, item, attach: s.attach }] };
  }
  if (words === s.startText) return null;
  const found = findText(currentProject(), s.id);
  if (!found) return words.trim() ? undefined : null;
  if (!words.trim()) return { label: 'text.remove.undo', ops: [{ type: 'removeItem', id: s.id }] };
  const content = found.item.content as Extract<Item['content'], { type: 'text' }>;
  return {
    label: 'text.edit.undo',
    ops: [{ type: 'updateItem', id: s.id, patch: { content: { ...content, text: words } } }],
  };
}

/** Whether finishing `s` now would commit something (bursts report it to undo, redo and closing). */
function commits(s: EditSession): boolean {
  const words = finalWords(s);
  return s.draft ? words.trim() !== '' : words !== s.startText;
}

/**
 * A commit that didn't go in: the typed words go to the clipboard. When the video changed meanwhile (or the layer is
 * gone) that is said, once, under the moved toast's key; any other failure was said by the edit itself.
 */
function keepWords(words: string, moved: boolean) {
  void navigator.clipboard?.writeText(words).catch(() => undefined);
  if (!moved) return;
  toast.info(
    t({
      id: 'textEdit.moved',
      message: 'The video changed while you typed. Your words are on the clipboard.',
      comment: 'Typing on the video couldn’t be saved: something else changed the video meanwhile',
    }),
    { key: 'project.moved' },
  );
}

/** The preview leaves the layer out while it is typed, or brings it back (null); a preview not running ignores it. */
const hide = (id: string | null) => engine.setEditing(id).catch(() => undefined);

/** Edits never begin in a project opened read-only (a newer Klipolo's): nothing could be saved. */
const readOnly = () => useProject.getState().snapshot?.readOnly != null;

export const useTextEdit = create<TextEditStore>()((set, get) => ({
  session: null,
  closing: null,
  begin: (id) => {
    const project = currentProject();
    const found = findText(project, id);
    if (!found || found.locked || readOnly() || get().session?.id === id) return;
    void get().finish();
    const playback = usePlayback.getState();
    if (playback.playing) playback.pause();
    useSelection.getState().select(id);
    // The editor shows at the playhead: "Edit text" on a title elsewhere goes to it first.
    const at = usePlayback.getState().playheadUs;
    if (at < found.item.startUs || at >= itemEndUs(found.item))
      usePlayback.getState().seek(showItemAtUs(found.item, project?.settings.fps ?? 30));
    const text = (found.item.content as { text: string }).text;
    const built = capture(useProject.getState().snapshot);
    set({ session: { id, draft: null, attach: false, text, startText: text, built, composing: false } });
    void hide(id);
  },
  beginDraft: (item, attach) => {
    if (readOnly()) return;
    void get().finish();
    const playback = usePlayback.getState();
    if (playback.playing) playback.pause();
    const text = item.content.type === 'text' ? item.content.text : '';
    const built = capture(useProject.getState().snapshot);
    set({ session: { id: item.id, draft: item, attach, text, startText: text, built, composing: false } });
  },
  // Capped only outside a composition: trimming mid-composition would cancel the input method.
  setText: (text) => {
    const s = get().session;
    if (s) set({ session: { ...s, text: s.composing ? text : capText(text) } });
  },
  setComposing: (on) => {
    const s = get().session;
    if (s) set({ session: { ...s, composing: on, text: on ? s.text : capText(s.text) } });
  },
  finish: async () => {
    const s = get().session;
    if (!s) return;
    const words = finalWords(s);
    const closing = { ...s, text: words };
    set({ session: null, closing });
    try {
      // Through the commit queue, so its op builds on a change still landing and what comes next builds on it. Kept at
      // close (said as what it would be now, if the close can't wait for it), never sent ahead of its turn: a whole
      // content built before a commit ahead of it lands would put that one back.
      const result = await new Promise<'ok' | 'gone' | 'failed'>((resolve) => {
        const planned = changeOf(s, words);
        // Nothing to commit (the words as they were, or an empty new box): it stays so at its turn, so it settles now,
        // and a close never leaves the title hidden behind it.
        if (planned === null) return resolve('ok');
        // A title gone meanwhile is kept too: at its turn its words go to the clipboard, said.
        const hold = keepAtClose(planned?.label ?? 'text.edit.undo');
        // The close gave up on it (it was said): its words go to the clipboard.
        hold.drop = () => resolve('failed');
        afterCommits(
          () => {
            hold.started = true;
            const change = changeOf(s, words);
            if (change === null || change === undefined) {
              hold.release();
              return resolve(change === null ? 'ok' : 'gone');
            }
            const end = beginCommit();
            // eslint-disable-next-line no-restricted-syntax -- sent at its turn (`afterCommits`), holding the queue
            useProject
              .getState()
              .apply(change.label, change.ops, undefined, s.built, { quietMoved: true })
              .then(
                (ok) => resolve(ok ? 'ok' : 'failed'),
                () => resolve('failed'),
              )
              .finally(() => {
                end();
                hold.release();
              });
          },
          { kept: hold },
        );
      });
      if (result === 'ok' && s.draft && words.trim()) useSelection.getState().select(s.draft.id);
      // Refused because the video moved since the edit began, or the layer is gone: said as such.
      if (result !== 'ok' && words.trim()) keepWords(words, result === 'gone' || changedSince(s.built));
      // Shown again only now: the engine has the new revision (or the old words stand), so nothing flashes. The
      // look stays drawn until the preview's next frames have the layer. A title begun meanwhile hid itself instead
      // (the preview leaves out one at a time), and shows itself again when it finishes.
      const next = get().session;
      if (!s.draft && !(next && !next.draft)) await hide(null);
      // The first frame after may still be a still without it (the new revision's, drawn before the unhide): two.
      await framesShown(2, HANDOFF_CAP_MS);
    } finally {
      if (get().closing === closing) set({ closing: null });
    }
  },
}));

// Playing finishes the edit first.
usePlayback.subscribe((s, prev) => {
  if (s.playing && !prev.playing) void useTextEdit.getState().finish();
});

// Closing the project (or quitting) finishes the edit first: its words go in their turn, kept at close, after any
// finished edit still waiting its turn, and the close waits for them. Undo, redo and a chat change don't end an edit:
// they can't start from the editor, and ending it mid-word would send the next keys to the editor's shortcuts.
registerBurst((closing) => {
  if (!closing) return false;
  const s = useTextEdit.getState().session;
  if (!s) return false;
  void useTextEdit.getState().finish();
  return commits(s);
});

/** Which opening of which project the copy holds. */
const opening = (p: { snapshot: { project: { id: string }; opening?: number } | null }) =>
  p.snapshot ? `${p.snapshot.project.id}:${p.snapshot.opening ?? ''}` : '';

// Another project (or none): no look of this one's edit stays on its stage, and an edit still open ends (refused by
// Rust as built for another project, its words go to the clipboard).
useProject.subscribe((s, prev) => {
  if (opening(s) === opening(prev)) return;
  const edit = useTextEdit.getState();
  if (edit.session) void edit.finish();
  useTextEdit.setState({ closing: null });
});

/** The engine started (or attached again) while a title is typed: it leaves that title out too. */
export function resendEditing() {
  const s = useTextEdit.getState().session;
  if (s && !s.draft) void hide(s.id);
}
