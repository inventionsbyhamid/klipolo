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
  create: (name: string, formats: FormatKind[]) => Promise<boolean>;
  /** The project file at `path` (the open one is saved and closed first). */
  open: (path: string) => Promise<boolean>;
  /** Every edit made goes in first (`commitsDone`), then the project closes once it's saved. */
  close: () => Promise<void>;
  /**
   * A project Rust already opened, as Rust has it now (the page loading on the project open at launch). Another project
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
  undo: () => Promise<void>;
  /**
   * Undo, only if the history still says `latest` at its turn (after a change still settling went in): chat's Undo chip
   * takes back its own change, never one made since. True when it undid.
   */
  undoIf: (latest: (history: HistoryState) => boolean) => Promise<boolean>;
  redo: () => Promise<void>;
}
