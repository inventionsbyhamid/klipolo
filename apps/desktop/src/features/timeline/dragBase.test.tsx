import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeEach, describe, expect, it } from 'vitest';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useUi } from '../../state/ui';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { commitLatest } from '../../state/commit';
import { Timeline } from './Timeline';

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));
const title = () => screen.getByRole('button', { name: /Set up billing/ });

/** A change from `from` to the next revision (the content stays the fixture's). */
const patch = (from: number): SnapshotPatch => ({
  baseRevision: from,
  history: { ...fixtureSnapshot().history, revision: from + 1 },
  patch: { clips: [], items: [] },
});

/** Rust's side: it refuses an edit whose base isn't its revision. */
function rust() {
  const state = { revision: 3, sent: [] as number[] };
  mockIPC((cmd, args) => {
    if (cmd === 'project_snapshot')
      return { ...fixtureSnapshot(), history: { ...fixtureSnapshot().history, revision: state.revision } };
    if (cmd !== 'project_apply') return null;
    const { baseRevision } = args as { baseRevision: number };
    state.sent.push(baseRevision);
    if (baseRevision !== state.revision) throw { code: 'project.moved', params: {} };
    state.revision += 1;
    return patch(baseRevision);
  });
  return state;
}

async function startDrag() {
  fireEvent.pointerDown(title(), { pointerId: 1, button: 0, clientX: 100, clientY: 10 });
  fireEvent.pointerMove(title(), { pointerId: 1, clientX: 160, clientY: 10 });
  await frames();
  expect(title().style.transform).toContain('60px');
}

async function release() {
  fireEvent.pointerUp(title(), { pointerId: 1, clientX: 160, clientY: 10 });
  await frames();
  await waitFor(() => expect(useProject.getState().pending).toBe(0));
}

describe('a timeline drag and changes landing mid-drag', () => {
  beforeEach(() => {
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    usePlayback.setState({ playheadUs: 25_000_000, playing: false });
    useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  });

  it('a chat change merged mid-drag stops the drag, and its release is refused, never added to the new position', async () => {
    const state = rust();
    renderWithProviders(<Timeline />);
    await startDrag();
    // Chat's change commits in Rust (3 → 4), and its reply's change merges here mid-drag.
    state.revision = 4;
    await act(() => useProject.getState().adopt(patch(3)));
    // The drag stops where it is: nothing shows its travel added to where chat's change put things.
    expect(title().style.transform).toBe('');
    fireEvent.pointerMove(title(), { pointerId: 1, clientX: 200, clientY: 10 });
    await frames();
    expect(title().style.transform).toBe('');
    await release();
    // Sent on the revision the drag began on: Rust refused it.
    expect(state.sent).toEqual([3]);
    expect(state.revision).toBe(4);
    expect(useProject.getState().snapshot!.history.revision).toBe(4);
  });

  it("the person's own commit landing mid-drag lets the release go in (no false refusal)", async () => {
    const state = rust();
    renderWithProviders(<Timeline />);
    await startDrag();
    // A toolbar change of the person's own goes in through the commit queue mid-drag (3 → 4).
    await act(() => commitLatest('text.add', () => [{ type: 'rename', name: 'Mine' }]));
    expect(title().style.transform).toContain('60px');
    await release();
    expect(state.sent).toEqual([3, 4]);
    expect(state.revision).toBe(5);
  });
});
