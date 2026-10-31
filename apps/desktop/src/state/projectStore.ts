import type { FormatKind } from '../bindings/FormatKind';
import type { HistoryState } from '../bindings/HistoryState';
import type { Op } from '../bindings/Op';
import type { Snapshot } from '../bindings/Snapshot';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { LabelId, LabelParams } from '../lib/labels';
import type { Built } from './editBase';

/** The open project as the UI holds it (`useProject`, `state/project.ts`), and what changes it. */
export interface ProjectStore {
  snapshot: Snapshot | null;
  pending: number;
  /** A create, open or close is under way, from the press until it lands or fails: another waits for it. */
  switching: boolean;
  /** A new project, opened: every edit made in the open one goes in first (`commitsDone`), and it is saved and closed. */
  create: (name: string, formats: FormatKind[]) => Promise<boolean>;
  /** The project file at `path`: every edit made in the open one goes in first, and it is saved and closed. */
  open: (path: string) => Promise<boolean>;
  /** Every edit made goes in first (`commitsDone`), then the project closes once it's saved. */
  close: () => Promise<void>;
  /**
   * A project Rust already opened, as Rust has it now (the page loading again with a project open). Another project
   * opens as any does; the open one takes it as a change from elsewhere, keeping the playhead, selection and preview
   * format, and keeping its own copy when that is newer.
   */
  load: (snapshot: Snapshot) => Promise<boolean>;
  /**
   * One undo step. `label` is its message id and `params` the values it shows (`'effect.add.undo'`, `{ effect: … }`):
   * history keeps them, never words, and the Undo item words them. `built`: where
   * `ops` were built (`captureBase`), when that was before this call (a held burst); else now. `quietMoved`: a refusal
   * because the project moved (`project.moved`) raises no toast of its own; the caller says it (on-canvas typing).
   */
  apply: (
    label: LabelId,
    ops: Op[],
    params?: LabelParams,
    built?: Built,
    opts?: { quietMoved?: boolean },
  ) => Promise<boolean>;
  /**
   * A change Rust already made (a chat edit): merged like an edit's patch, after every command queued before it. False
   * when it couldn't be (the project was fetched again instead, or that failed).
   */
  adopt: (change: SnapshotPatch) => Promise<boolean>;
  /**
   * An edit Rust builds from the project as it is (a media command: music that fits the video, a title template): sent
   * with the copy's base like `apply` (`built`: where it was asked for, else now), its change merged the same way. False
   * when it was refused or the project is read-only.
   */
  send: (task: () => Promise<SnapshotPatch>, built?: Built) => Promise<boolean>;
  undo: () => Promise<void>;
  /**
   * Undo, only if the history still says `latest` at its turn (after a change still settling went in): the import
   * toast's Undo takes back its own import, never a change made since. True when it undid.
   */
  undoIf: (latest: (history: HistoryState) => boolean) => Promise<boolean>;
  /**
   * A change Rust makes for chat (its Undo chip's `chat_undo_last`, Use's `chat_use_picture`, Apply's
   * `chat_apply_proposal`) in undo's turn: what is still settling goes in first, it waits its turn in the commit queue,
   * and the patch `task` gives back is merged. Rust's refusal comes back to the caller as it is (it rejects): nothing
   * is fetched again and nothing said, since there `project.moved` means the change can't be made from chat any more,
   * not that this copy is old. `undo`: it is an undo (the chip), counted in `historySteps`; Use and Apply aren't. True
   * when it landed.
   */
  stepWith: (task: () => Promise<SnapshotPatch>, opts?: { undo?: boolean }) => Promise<boolean>;
  redo: () => Promise<void>;
}
