import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { mockIPC } from '@tauri-apps/api/mocks';
import { beforeAll, describe, expect, it } from 'vitest';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { Stage } from './Stage';

beforeAll(() => {
  // jsdom has no layout: give the stage an 800×450 box.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get: () => 450 });
});

describe('a canvas drag and a change from elsewhere', () => {
  it('a chat change merged mid-drag makes the release refused, never the layer put back as it was', async () => {
    // Rust's side: it refuses an edit whose base isn't its revision.
    const rust = { revision: 3, sent: [] as (number | null)[] };
    mockIPC((cmd, args) => {
      if (cmd === 'project_snapshot')
        return { ...fixtureSnapshot(), history: { ...fixtureSnapshot().history, revision: rust.revision } };
      if (cmd !== 'project_apply') return null;
      const { baseRevision } = args as { baseRevision: number };
      rust.sent.push(baseRevision);
      if (baseRevision !== rust.revision) throw { code: 'project.moved', params: {} };
      rust.revision += 1;
      return null;
    });
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    useSelection.setState({ selectedId: 'title' });
    // The title shows from 1 s to 4 s.
    usePlayback.setState({ playheadUs: 2_000_000, playing: false, seq: 0 });
    renderWithProviders(<Stage />);
    const gizmo = () => screen.getByRole('button', { name: /^Text: Set up billing/ });
    // The person starts moving the title at revision 3…
    fireEvent.pointerDown(gizmo(), { button: 0, clientX: 100, clientY: 100, pointerId: 1 });
    fireEvent.pointerMove(gizmo(), { clientX: 180, clientY: 100, pointerId: 1 });
    // …while chat's "make the title bigger" commits (3 → 4) and its reply's change merges here.
    rust.revision = 4;
    const chat: SnapshotPatch = {
      baseRevision: 3,
      history: { ...fixtureSnapshot().history, revision: 4 },
      patch: { clips: [], items: [] },
    };
    await act(() => useProject.getState().adopt(chat));
    fireEvent.pointerUp(gizmo(), { pointerId: 1 });
    // The release goes on the revision the drag began on, and Rust refuses it.
    await waitFor(() => expect(rust.sent).toEqual([3]));
    await waitFor(() => expect(useProject.getState().pending).toBe(0));
    expect(rust.revision).toBe(4);
  });
});
