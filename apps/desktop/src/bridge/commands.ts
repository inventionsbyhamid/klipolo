import { convertFileSrc } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { FormatKind } from '../bindings/FormatKind';
import type { MediaImport } from '../bindings/MediaImport';
import type { Snapshot } from '../bindings/Snapshot';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { Transaction } from '../bindings/Transaction';
import { applyBase } from './applyBase';
import { call } from './call';

/**
 * A save landed (`ok: true`, with the opening and revision it wrote), or saves started failing (`ok: false`, with what
 * went wrong and what to do).
 */
export interface SaveStatus {
  ok: boolean;
  message?: string;
  opening?: number;
  revision?: number;
}

/** The project commands (`crates/klipolo-app/src/project.rs`) and media import (`media.rs`). */
export const bridge = {
  /** A new project in `~/Movies/Klipolo`, opened (the open one is saved and closed first). */
  createProject: (name: string, formats: FormatKind[]) => call<Snapshot>('project_create', { name, formats }),
  /** The project file at `path`, opened (the open one is saved and closed first). */
  openProject: (path: string) => call<Snapshot>('project_open', { path }),
  /** Closes the open project once it's saved. */
  closeProject: () => call<void>('project_close'),
  /**
   * Edits, undo and redo return what changed, on the revision it was made on. An edit goes with the project and
   * revision the UI's copy holds when it is sent (`applyBase`): Rust refuses a stale one with `project.moved` and
   * never rebases it.
   */
  apply: (tx: Transaction) => call<SnapshotPatch>('project_apply', { tx, ...applyBase() }),
  /**
   * Undo and redo go with the copy's revision too: Rust refuses them when its history's top is a change not seen
   * yet.
   */
  undo: () => call<SnapshotPatch>('project_undo', { ...applyBase() }),
  redo: () => call<SnapshotPatch>('project_redo', { ...applyBase() }),
  /** The whole open project, when a change doesn't follow the revision the UI has (and when the page loads). */
  snapshot: () => call<Snapshot>('project_snapshot'),
  /** Probes `paths` (a Finder drop), or the files the person picks (none given): the assets, and what was skipped. */
  importMedia: (paths?: string[]) => call<MediaImport>('media_import', { paths: paths ?? null }),
  /** Edits are saved in the background: this hears each save that lands, and when saving starts failing. */
  onSaveStatus: (handler: (status: SaveStatus) => void): Promise<UnlistenFn> =>
    listen<SaveStatus>('project-save', (event) => handler(event.payload)),
};

export function mediaUrl(path: string): string {
  return convertFileSrc(path);
}
