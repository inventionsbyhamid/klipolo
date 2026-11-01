import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import { renderWithProviders } from '../../test/render';
import { twoTakesSnapshot } from '../../test/sounds';

/*
 * The clip card's Sounds: a recording clip's sounds, each muted and set in this clip alone, each change one step.
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../../bridge/framing', () => ({
  clipFraming: vi.fn(() => new Promise(() => undefined)),
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { Inspector } = await import('./Inspector');

const lastApply = async () => {
  await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalled());
  const [tx] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
  return { label: tx.label, ops: tx.ops };
};

function card(clipId: string, snapshot: Snapshot = twoTakesSnapshot()) {
  useProject.setState({ snapshot, pending: 0 });
  act(() => useSelection.setState({ selectedId: clipId }));
  renderWithProviders(<Inspector format="landscape" />);
  return screen.getByRole('group', { name: 'Sounds' });
}

const row = (sounds: HTMLElement, id: string) => sounds.querySelector<HTMLElement>(`[data-source-row="${id}"]`)!;

beforeEach(() => {
  useSelection.setState({ selectedId: null });
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

describe("a recording clip's Sounds", () => {
  it('lists each recorded sound with its own volume and mute, and the whole-clip Volume stays', () => {
    const sounds = card('c2');
    expect(within(row(sounds, 'mic')).getByText('Mic')).toBeInTheDocument();
    expect(within(sounds).getByRole('slider', { name: 'Mic volume' })).toHaveAttribute('aria-valuenow', '0.6');
    expect(within(sounds).getByRole('slider', { name: 'Computer sound volume' })).toBeInTheDocument();
    expect(within(sounds).getByRole('button', { name: 'Mute Mic in this clip' })).not.toHaveAttribute('aria-pressed');
    expect(row(sounds, 'screen')).toBeNull();
    expect(screen.getByRole('slider', { name: 'Volume' })).toBeInTheDocument();
  });

  it('mutes one sound in this clip alone, in one step', async () => {
    const sounds = card('c3');
    await userEvent.click(within(sounds).getByRole('button', { name: 'Mute Mic in this clip' }));
    expect(await lastApply()).toEqual({
      label: 'undo.source.mute',
      ops: [
        {
          type: 'setClipSourceMix',
          clipId: 'c3',
          sourceId: 'mic',
          mix: { volume: 1, muted: true, effects: [], mutedByPerson: true },
        },
      ],
    });
  });

  it("sets one sound's volume in this clip alone, in one step", async () => {
    const sounds = card('c3');
    const mic = within(sounds).getByRole('slider', { name: 'Mic volume' });
    mic.focus();
    fireEvent.keyDown(mic, { key: 'ArrowLeft' });
    fireEvent.blur(mic);
    expect(await lastApply()).toEqual({
      label: 'undo.source.volume',
      ops: [{ type: 'setClipSourceMix', clipId: 'c3', sourceId: 'mic', mix: { volume: 0.99, muted: false, effects: [] } }],
    });
  });

  it('holds a separated sound as it is in this clip, and says why', async () => {
    const sounds = card('c1');
    const mic = row(sounds, 'mic');
    expect(within(mic).getByRole('slider', { name: 'Mic volume' })).toHaveAttribute('aria-disabled', 'true');
    const mute = within(mic).getByRole('button', { name: 'Unmute Mic in this clip' });
    expect(mute).toHaveAttribute('aria-disabled', 'true');
    expect(within(mic).getByText('Mic is on its own track here.')).toBeInTheDocument();
    await userEvent.click(mute);
    await new Promise((r) => setTimeout(r, 20));
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it("moves a sound 10 ms earlier against the picture, for the whole recording", async () => {
    const sounds = card('c2');
    const mic = row(sounds, 'mic');
    expect(within(mic).getByText('In sync as recorded')).toBeInTheDocument();
    await userEvent.click(within(mic).getByRole('button', { name: 'Move Mic 10 ms earlier' }));
    expect(await lastApply()).toEqual({
      label: 'undo.source.sync',
      ops: [{ type: 'updateRecordingSource', recordingId: 'r1', sourceId: 'mic', patch: { nudgeUs: -10_000 } }],
    });
  });

  it('says how far a sound is moved, and that its separated sound moves with it', () => {
    const snap = twoTakesSnapshot();
    snap.project.recordings[0].sources[1].nudgeUs = -40_000;
    const mic = row(card('c2', snap), 'mic');
    expect(within(mic).getByText('40 ms earlier')).toBeInTheDocument();
    expect(
      within(mic).getByText('Its separated sound moves with it. Its volume and mute stay its own.'),
    ).toBeInTheDocument();
  });

  it("has no Sounds for a clip of an imported file", () => {
    const snap = twoTakesSnapshot();
    snap.project.timeline.main.push({ ...snap.project.timeline.main[2], id: 'b', source: { type: 'asset', id: 'v2' } });
    useProject.setState({ snapshot: snap, pending: 0 });
    act(() => useSelection.setState({ selectedId: 'b' }));
    renderWithProviders(<Inspector format="landscape" />);
    expect(screen.queryByRole('group', { name: 'Sounds' })).toBeNull();
  });
});
