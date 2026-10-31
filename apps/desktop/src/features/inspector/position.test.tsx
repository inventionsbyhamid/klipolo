import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The card's Position grid in 9:16, where its places are inside the safe area: it checks the place a layer is at, so a
 * new video layer (bottom right of the safe area) reads Bottom right, and so does a picture placed there.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { dropOps } = await import('../timeline/drops');
const { Inspector } = await import('./Inspector');

/** The fixture with `primary` as its main format, and `asset` dropped as a layer at 1 s (as the library adds one). */
function dropped(primary: FormatKind, asset: string) {
  const snapshot = fixtureSnapshot();
  const { project } = snapshot;
  project.settings = { ...project.settings, formats: ['landscape', 'portrait'], primaryFormat: primary };
  const file = project.assets.find((a) => a.id === asset)!;
  const [op] = dropOps(project, file, { lane: 'picture', trackId: null, atUs: 1_000_000 }, 30);
  if (op.type !== 'addItem') throw new Error('no layer');
  const item: Item = { ...op.item, id: 'layer' };
  project.timeline.tracks[0].items.push(item);
  useProject.setState({ snapshot, pending: 0, apply });
  useSelection.setState({ selectedId: 'layer' });
  return snapshot;
}

const checked = () => screen.getAllByRole('radio', { checked: true }).map((r) => r.getAttribute('aria-label'));
const layoutOf = (call: Parameters<typeof apply>) =>
  (call[1][0] as unknown as { patch: { layout: Layout } }).patch.layout;

beforeEach(() => {
  vi.clearAllMocks();
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
});

describe('Position in 9:16', () => {
  it('a new video layer reads Bottom right, and another place moves it there', async () => {
    dropped('portrait', 'broll');
    renderWithProviders(<Inspector format="portrait" />);
    expect(checked()).toContain('Bottom right');
    expect(checked()).not.toContain('Middle');
    await userEvent.click(screen.getByRole('radio', { name: 'Top left' }));
    expect(apply.mock.calls[0][0]).toBe('layout.position.undo');
    // The safe area's top left (x 6 %, y 14 %), 4 % of the 1080 px side in.
    const { rect } = layoutOf(apply.mock.calls[0]);
    expect(rect.x).toBeCloseTo(0.06 + 0.04);
    expect(rect.y).toBeCloseTo(0.14 + (0.04 * 1080) / 1920);
  });

  it('a new video layer in a 16:9 project, shown in 9:16, reads Bottom right there too', () => {
    dropped('landscape', 'broll');
    renderWithProviders(<Inspector format="portrait" />);
    expect(checked()).toContain('Bottom right');
  });

  it('a picture placed at Bottom right reads Bottom right', async () => {
    const snapshot = dropped('portrait', 'logo');
    renderWithProviders(<Inspector format="portrait" />);
    expect(checked()).toContain('Middle');
    await userEvent.click(screen.getByRole('radio', { name: 'Bottom right' }));
    const layout = layoutOf(apply.mock.calls[0]);
    const next = structuredClone(snapshot);
    next.project.timeline.tracks[0].items = next.project.timeline.tracks[0].items.map((item) =>
      item.id === 'layer' ? { ...item, layout } : item,
    );
    act(() => useProject.setState({ snapshot: next }));
    expect(checked()).toContain('Bottom right');
  });
});
