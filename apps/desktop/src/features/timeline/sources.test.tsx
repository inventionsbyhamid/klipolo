import { fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { twoTakesSnapshot } from '../../test/sounds';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { kindMuteOps, kindVolumeOps } = await import('./keys/sourceMix');

/** The last committed batch: (label, ops). */
const lastApply = async () => {
  await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalled());
  const [tx] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
  return { label: tx.label, params: tx.params, ops: tx.ops };
};

const lanes = () => screen.getAllByRole('group').map((g) => g.getAttribute('aria-label'));

function show(snapshot: Snapshot) {
  useProject.setState({ snapshot, pending: 0 });
  renderWithProviders(<Timeline />);
}

beforeEach(() => {
  usePlayback.setState({ playheadUs: 0, playing: false });
  useSelection.setState({ selectedId: null });
  useUi.setState({ pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply)
    .mockReset()
    .mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [] },
      };
    });
});

describe('the sound lanes under Video', () => {
  it('shows one lane per kind of recorded sound, not one per take, right under Video', () => {
    show(twoTakesSnapshot());
    expect(lanes()).toEqual(['Overlays', 'Video', 'Mic', 'Computer sound', 'Sound effects', 'Music']);
  });

  it('shows a kind only while a take the video plays has it', () => {
    const snap = twoTakesSnapshot();
    // Only r2 (screen and mic) plays: no Computer sound lane.
    snap.project.timeline.main = snap.project.timeline.main.filter((c) => c.id === 'c3');
    show(snap);
    expect(lanes()).toEqual(['Overlays', 'Video', 'Mic', 'Sound effects', 'Music']);
  });

  it('shows none without a recording', () => {
    const snap = fixtureSnapshot();
    snap.project.recordings = [];
    snap.project.timeline.main = snap.project.timeline.main.filter((c) => c.source.type === 'asset');
    show(snap);
    expect(lanes()).toEqual(['Overlays', 'Video', 'Sound effects', 'Music']);
  });

  it("gives a take's second mic Klipolo named its own lane, Mic 2", () => {
    const snap = twoTakesSnapshot();
    const r1 = snap.project.recordings[0];
    r1.sources.push({ ...r1.sources[1], id: 'mic-2', runs: [{ ...r1.sources[1].runs[0], assetId: 's1' }] });
    show(snap);
    expect(lanes()).toEqual(['Overlays', 'Video', 'Mic', 'Mic 2', 'Computer sound', 'Sound effects', 'Music']);
  });

  it("draws each clip's part of the sound as a waveform only: nothing there to select", () => {
    show(twoTakesSnapshot());
    const mic = screen.getByRole('group', { name: 'Mic' });
    const parts = mic.querySelectorAll('[data-source-part]');
    expect(parts).toHaveLength(3);
    for (const part of parts) expect(part).toHaveAttribute('aria-hidden', 'true');
    expect(mic.querySelector('[data-block-id]')).toBeNull();
    // c1's mic plays from its own layer: its part is dimmed, as a muted sound's is.
    expect(parts[0]).toHaveAttribute('data-muted', 'true');
    expect(parts[1]).not.toHaveAttribute('data-muted');
  });
});

describe("a sound lane's header", () => {
  it('mutes that kind of sound in every recording, in one step', async () => {
    show(twoTakesSnapshot());
    const mute = within(screen.getByRole('group', { name: 'Mic' })).getByRole('button', { name: 'Mute Mic' });
    // Its name says what it does next, so it has no pressed state as well.
    expect(mute).not.toHaveAttribute('aria-pressed');
    await userEvent.click(mute);
    const { label, params, ops } = await lastApply();
    expect(label).toBe('undo.source.mute');
    expect(params).toEqual({ source: { code: 'source.role.mic', params: {} } });
    expect(ops).toEqual(kindMuteOps(twoTakesSnapshot().project, 'mic', true));
    expect(ops.map((op) => ('recordingId' in op ? op.recordingId : 'clipId' in op ? op.clipId : null))).toEqual([
      'r1',
      'c2',
      'r2',
    ]);
  });

  it('shows Unmute once every recording has it muted', () => {
    const snap = twoTakesSnapshot();
    for (const r of snap.project.recordings) r.sources.find((s) => s.role === 'mic')!.mix.muted = true;
    show(snap);
    const lane = screen.getByRole('group', { name: 'Mic' });
    expect(within(lane).getByRole('button', { name: 'Unmute Mic' })).toBeInTheDocument();
    // c2 plays the mic at its own 60 %, which a take's default doesn't change: only its part isn't dimmed.
    const dimmed = [...lane.querySelectorAll('[data-source-part]')].map((part) => part.hasAttribute('data-muted'));
    expect(dimmed).toEqual([true, false, true]);
  });

  it("sets that kind of sound's volume in every recording from its popover, in one step", async () => {
    show(twoTakesSnapshot());
    const lane = screen.getByRole('group', { name: 'Computer sound' });
    await userEvent.click(within(lane).getByRole('button', { name: 'Computer sound volume' }));
    const slider = await screen.findByRole('slider', { name: 'Volume' });
    expect(screen.getByText('For every clip, also those with their own setting.')).toBeInTheDocument();
    slider.focus();
    fireEvent.keyDown(slider, { key: 'ArrowLeft' });
    fireEvent.blur(slider);
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.volume');
    expect(ops).toEqual(kindVolumeOps(twoTakesSnapshot().project, 'systemAudio', 0.99));
  });

  it("says a separated part keeps its own volume", async () => {
    show(twoTakesSnapshot());
    const lane = screen.getByRole('group', { name: 'Mic' });
    await userEvent.click(within(lane).getByRole('button', { name: 'Mic volume' }));
    expect(
      await screen.findByText(
        'For every clip, also those with their own setting. Its separated sound keeps its own volume.',
      ),
    ).toBeInTheDocument();
  });
});
