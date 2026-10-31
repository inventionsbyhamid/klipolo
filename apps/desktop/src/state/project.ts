import { create } from 'zustand';
import type { Snapshot } from '../bindings/Snapshot';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { withBase } from '../bridge/applyBase';
import { codedMessage, isCode } from '../bridge/call';
import { bridge } from '../bridge/commands';
import { errorMessage } from '../lib/errors';
import { applyPatch } from '../lib/patch';
import { flushBursts } from './bursts';
import { afterCommits, beginCommit, commitsDone, yieldToSettling } from './commitQueue';
import { baseFor, capture, changedElsewhere, type Built } from './editBase';
import { serial } from './projectSerial';
import type { ProjectStore } from './projectStore';
import { setFrameRateSource, usePlayback } from './playback';
import { useSelection } from './selection';
import { toast } from './toasts';
import { useUi } from './ui';

export { settled } from './projectSerial';

/** Undo and redo steps asked for in this window (a press the word list kept waiting goes only if none came). */
let steps = 0;
export const historySteps = () => steps;

/** A different project (or none) starts at the beginning, with nothing selected, in its primary format. */
function resetView() {
  usePlayback.getState().reset();
  useSelection.getState().select(null);
  useUi.getState().setPreviewFormat(null);
}

export const useProject = create<ProjectStore>((set, get) => {
  /** A newer Klipolo's project opens read-only: it said so once on open, so edits just don't happen. */
  const readOnly = () => get().snapshot?.readOnly != null;

  /**
   * The change lands with `pending` back down in the same store change: two changes, not three. An edit Rust refused
   * because another change landed first (`project.moved`: chat) takes the project as it is now, in its turn, and says
   * so (unless `quietMoved`: the caller says it in its own words): false, as nothing the person did was applied.
   */
  async function run(
    task: () => Promise<Snapshot | null>,
    switching = false,
    quietMoved = false,
  ): Promise<boolean> {
    set((s) => ({ pending: s.pending + 1 }));
    let snapshot: Snapshot | null;
    let moved = false;
    try {
      snapshot = await serial(async () => {
        try {
          return await task();
        } catch (error) {
          if (!isCode(error, 'project.moved')) throw error;
          moved = true;
          changedElsewhere();
          // What is still settling was built on the copy before: it goes now, and is refused the same way.
          flushBursts();
          const now = await bridge.snapshot();
          // Again once it is here: an edit built while it was on its way was built on the copy before it.
          changedElsewhere();
          return now;
        }
      });
    } catch (error) {
      set((s) => ({ pending: s.pending - 1 }));
      // Closing or switching while saves keep failing says why (a full disk), and the project stays open.
      toast.error(errorMessage(error));
      return false;
    }
    if (switching) {
      resetView();
      // Rust sends the notice as a code (`model.readOnly`), worded here.
      if (snapshot?.readOnly)
        toast.info(codedMessage({ code: snapshot.readOnly, params: {} }) || snapshot.readOnly);
    }
    // Another project (or none): an edit built for the one before is refused, never sent to this one. Counted before
    // the store changes, so what watches it (a drag) already sees it as a change from elsewhere.
    if (switching) changedElsewhere();
    set((s) => ({ snapshot, pending: s.pending - 1 }));
    // Said once however many queued edits it refused.
    if (moved && !quietMoved)
      toast.info(codedMessage({ code: 'project.moved', params: {} }), { key: 'project.moved' });
    return !moved;
  }

  /**
   * A change from Rust: applied to the revision it was made on, else the whole project is fetched again. So is a
   * patch naming a clip or item this copy doesn't have: a resync beats silently dropping it.
   */
  async function merge(change: SnapshotPatch): Promise<Snapshot | null> {
    const current = get().snapshot;
    if (!current) return null;
    const project =
      change.baseRevision === current.history.revision ? applyPatch(current.project, change.patch) : null;
    // The opening stays: a patch is a change to this one.
    if (project)
      return { project, history: change.history, readOnly: change.readOnly, opening: current.opening };
    // The project as Rust has it: it may hold changes this copy never saw (counted before and once it is here).
    changedElsewhere();
    const now = await bridge.snapshot();
    changedElsewhere();
    return now;
  }

  /** Sends an edit built at `built` with its base (`baseFor`): read inside the serialised task, when it is sent. */
  function sendFrom(built: Built, send: () => Promise<SnapshotPatch>, quietMoved = false): Promise<boolean> {
    return edit(() => withBase(baseFor(built, get().snapshot), send), quietMoved);
  }

  /** An edit, undo or redo: serialised like `run`, so the store holds the revision the change was made on. */
  function edit(task: () => Promise<SnapshotPatch>, quietMoved = false): Promise<boolean> {
    return run(async () => merge(await task()), false, quietMoved);
  }

  /**
   * Undo or redo. A change still settling commits first; then it waits its turn in the commit queue, behind a slider
   * release or a pick still waiting, and holds the queue until it lands. So it takes back the last change made, and an
   * edit made meanwhile builds on it. A nudge a press waiting ahead of it starts goes in first too (`yieldToSettling`):
   * `.` then ⌘Z within one commit's latency takes back the nudge. `check`, at its turn, can call it off. It goes with
   * the copy's revision at its turn, so Rust refuses it (`project.moved`) when its history's top is a change this
   * window hasn't seen (a chat change not merged yet). Resolves once it has landed (true when it applied), false when
   * called off or refused. A close waits for it in its turn (`commitsDone`).
   */
  function step(task: () => Promise<SnapshotPatch>, check?: () => boolean): Promise<boolean> {
    steps++;
    flushBursts();
    return new Promise((resolve) => {
      const turn = () => {
        if (yieldToSettling(turn)) return;
        if (readOnly() || (check && !check())) return resolve(false);
        const end = beginCommit();
        void sendFrom(capture(get().snapshot), task).then((done) => {
          end();
          resolve(done);
        });
      };
      afterCommits(turn);
    });
  }

  return {
    snapshot: null,
    pending: 0,
    create: (name, formats) => run(() => bridge.createProject(name, formats), true),
    open: (path) => run(() => bridge.openProject(path), true),
    load: (snapshot) => {
      if (get().snapshot?.project.id !== snapshot.project.id) return run(async () => snapshot, true);
      // As `adopt`: what is still settling goes first (Rust refuses it: it was built on the copy before), and it holds
      // the commit queue until the project lands.
      flushBursts();
      const end = beginCommit();
      return run(async () => {
        changedElsewhere();
        // A change merged after Rust made this snapshot (a chat reply that landed meanwhile) is in the copy already:
        // an older snapshot of the same opening never replaces a newer copy.
        const now = get().snapshot;
        if (now && now.opening === snapshot.opening && snapshot.history.revision < now.history.revision)
          return now;
        return snapshot;
      }).finally(end);
    },
    close: async () => {
      // Every edit made goes in before the project closes, as at quit: what is still settling, every commit on its way
      // and every edit waiting its turn (a press's, a queued drop, an undo), each in its turn.
      flushBursts({ closing: true });
      await commitsDone();
      await run(async () => {
        await bridge.closeProject();
        return null;
      }, true);
    },
    apply: (label, ops, params, built, opts) => {
      if (readOnly()) return Promise.resolve(false);
      if (ops.length === 0) return Promise.resolve(true);
      // Params only when there are some: the transaction leaves them out otherwise.
      const tx = params && Object.keys(params).length > 0 ? { label, params, ops } : { label, ops };
      // Without `built`, the caller built `ops` from the copy as it is now.
      return sendFrom(built ?? capture(get().snapshot), () => bridge.apply(tx), opts?.quietMoved);
    },
    adopt: (change) => {
      // A change still settling was built on the copy before this one: it goes first, so Rust refuses it.
      flushBursts();
      // It holds the commit queue until it lands, so a drop or nudge waiting there builds on the changed project.
      const end = beginCommit();
      return run(async () => {
        const next = await merge(change);
        changedElsewhere();
        return next;
      }).finally(end);
    },
    // A toolbar change still settling commits first (it queues ahead), so undo takes back exactly that change.
    undo: () => step(() => bridge.undo()).then(() => undefined),
    undoIf: (latest) =>
      step(
        () => bridge.undo(),
        () => {
          const history = get().snapshot?.history;
          return history !== undefined && latest(history);
        },
      ),
    redo: () => step(() => bridge.redo()).then(() => undefined),
    stepWith: (task) => {
      let refused: { error: unknown } | null = null;
      return step(async () => {
        try {
          return await task();
        } catch (error) {
          refused = { error };
          // Nothing changed: the copy's own revision, with no patch.
          const now = get().snapshot!;
          return { baseRevision: now.history.revision, history: now.history, patch: { clips: [], items: [] } };
        }
      }).then((done) => {
        if (refused) throw refused.error;
        return done;
      });
    },
  };
});

/** Where an edit built now is built (`Built`): for ops built before they go in, such as a held burst's. */
export function captureBase(): Built {
  return capture(useProject.getState().snapshot);
}

// Playback anchors its clock on the open project's frame grid.
setFrameRateSource(() => useProject.getState().snapshot?.project.settings.fps ?? 30);
