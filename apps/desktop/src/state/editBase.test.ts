import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Op } from '../bindings/Op';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { fixtureSnapshot } from '../test/fixtures';
import { commitInFlight } from './commitQueue';
import { keyBurst } from './keyBurst';
import { useProject } from './project';

const OPS: Op[] = [{ type: 'removeItem', id: 'title' }];
const moved = { code: 'project.moved', params: {} };

/** A change from `from` to the next revision (the project's content is the fixture's throughout). */
function patch(from: number): SnapshotPatch {
  return {
    baseRevision: from,
    history: { ...fixtureSnapshot().history, revision: from + 1 },
    patch: { clips: [], items: [] },
  };
}

/**
 * Rust's side of `project_apply`, `project_undo` and `project_redo`: each refuses a base that isn't its revision.
 * `hold` makes the edits sent at those positions (0-based) wait until released, as a slow commit does; `slowSnapshot`
 * holds the first resync the same way.
 */
function rust(start: number, hold: number[] = [], { slowSnapshot = false } = {}) {
  const state = { revision: start, sent: [] as (number | null)[], undone: [] as (number | null)[] };
  const releases: (() => void)[] = [];
  let snapshots = 0;
  mockIPC((cmd, args) => {
    const { baseRevision } = (args ?? {}) as { baseRevision?: number | null };
    if (cmd === 'project_snapshot') {
      const now = {
        ...fixtureSnapshot(),
        history: { ...fixtureSnapshot().history, revision: state.revision },
      };
      if (!slowSnapshot || snapshots++ > 0) return now;
      return new Promise((resolve) =>
        releases.push(() => resolve({ ...now, history: { ...now.history, revision: state.revision } })),
      );
    }
    if (cmd === 'project_undo') {
      state.undone.push(baseRevision ?? null);
      // No base: the top step as it is (Rust's None).
      if (baseRevision != null && baseRevision !== state.revision) throw moved;
      state.revision += 1;
      return patch(state.revision - 1);
    }
    if (cmd !== 'project_apply') return null;
    state.sent.push(baseRevision ?? null);
    if (baseRevision !== state.revision) throw moved;
    state.revision += 1;
    const landed = patch(baseRevision);
    if (!hold.includes(state.sent.length - 1)) return landed;
    return new Promise((resolve) => releases.push(() => resolve(landed)));
  });
  /** Waits until Rust has the held edit (IPC is asynchronous), then lets it land. */
  const arrived = () => vi.waitFor(() => expect(releases.length).toBeGreaterThan(0));
  const release = async () => {
    await arrived();
    releases.shift()!();
  };
  return { state, arrived, release };
}

describe('the revision an edit is sent on: a stale one is refused, never rebased', () => {
  beforeEach(() => useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 }));

  it('own queued edits follow each other; one built before a chat change merged is refused', async () => {
    const { state, arrived, release } = rust(3, [0, 2]);
    const { apply, adopt } = useProject.getState();
    // Two of this window's own edits, both built at 3: the second follows the first (queued commits).
    const first = apply('text.add', OPS);
    const second = apply('text.add', OPS);
    await release();
    expect(await Promise.all([first, second])).toEqual([true, true]);
    expect(state.sent).toEqual([3, 4]);
    // A slow edit at 5; meanwhile Rust commits chat's change after it (6 → 7), and its reply's change waits to merge.
    const third = apply('text.add', OPS);
    const chat = adopt(patch(6));
    // The commit queue holds drops and nudges until chat's change has merged.
    expect(commitInFlight()).toBe(true);
    // Built at 5, before chat's change reached this copy: its ops are of the project before the cut.
    const fourth = apply('text.add', OPS);
    await arrived();
    state.revision += 1;
    await release();
    expect(await Promise.all([third, chat, fourth])).toEqual([true, true, false]);
    expect(state.sent).toEqual([3, 4, 5, 5]);
    expect(commitInFlight()).toBe(false);
    expect(useProject.getState().snapshot!.history.revision).toBe(7);
  });

  it('after a refusal resyncs, an edit built before it is refused too, never applied on the new project', async () => {
    // Rust is at 5 (a chat change this copy hasn't heard of yet); two edits built at 3.
    const { state } = rust(5);
    const { apply } = useProject.getState();
    const results = await Promise.all([apply('text.add', OPS), apply('text.add', OPS)]);
    expect(results).toEqual([false, false]);
    expect(state.sent).toEqual([3, 3]);
    expect(useProject.getState().snapshot!.history.revision).toBe(5);
    // Built now, on the project as it is: it goes in.
    expect(await useProject.getState().apply('text.add', OPS)).toBe(true);
    expect(state.sent).toEqual([3, 3, 5]);
  });

  it('an edit built while a refusal resyncs is refused too, never sent on the resynced revision (probe D)', async () => {
    // Rust is at 5; this copy at 3. The refusal's resync takes a while.
    const { state, release } = rust(5, [], { slowSnapshot: true });
    const { apply } = useProject.getState();
    const first = apply('text.add', OPS);
    await vi.waitFor(() => expect(state.sent).toEqual([3]));
    // The gizmo, Add text or the format toggle (none waits on the commit queue): built on the copy at 3.
    const during = apply('text.add', OPS);
    await release();
    expect(await Promise.all([first, during])).toEqual([false, false]);
    expect(state.sent).toEqual([3, 3]);
    expect(useProject.getState().snapshot!.history.revision).toBe(5);
  });

  it('a held nudge built before a chat change merged is refused (probe E)', async () => {
    // The nudge's ops are built at the press, on the copy at 3; it settles for 400 ms.
    const { state } = rust(3);
    keyBurst.push('text.add', OPS, 'nudge:title');
    // Meanwhile Rust commits chat's change (3 → 4), and its reply's change merges here.
    state.revision = 4;
    await useProject.getState().adopt(patch(3));
    keyBurst.flush();
    await vi.waitFor(() => expect(useProject.getState().pending).toBe(0));
    // It went on the revision it was built on, and Rust refused it.
    expect(state.sent).toEqual([3]);
    expect(state.revision).toBe(4);
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
  });


  it('Undo goes with the copy’s revision: a chat change not merged yet is never what it takes back', async () => {
    const { state } = rust(5);
    // Rust's history has a chat change on top (5) that this copy (3) hasn't merged.
    await useProject.getState().undo();
    expect(state.undone).toEqual([3]);
    expect(state.revision).toBe(5);
    expect(useProject.getState().snapshot!.history.revision).toBe(5);
    // On the resynced copy, it takes back that change.
    await useProject.getState().undo();
    expect(state.undone).toEqual([3, 5]);
    expect(state.revision).toBe(6);
  });
});
