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
vi.mock('../../bridge/media', () => ({
  media: { peaksInfo: vi.fn(async () => null), channelPeaks: vi.fn(async () => null), thumbs: vi.fn(async () => null) },
}));
const { bridge } = await import('../../bridge/commands');
const { media } = await import('../../bridge/media');
const { useProject } = await import('../../state/project');
const { useMedia } = await import('../../state/media');
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

describe("a two-sided sound's Centre switch", () => {
  /** The sound cache says the mic's file has `channels` channels, and the file is loudest on its right. */
  function twoSided(channels: number) {
    vi.mocked(media.peaksInfo).mockImplementation(async (path) =>
      path === '/media/m1' ? { channels, levels: 1, bucket: 256, frames: 48_000 } : null,
    );
    vi.mocked(media.channelPeaks).mockImplementation(async (path) => (path === '/media/m1' ? [0.001, 0.6] : null));
    // A new media version: the header is read again, not taken from an earlier test's.
    useMedia.getState().bump('/media/m1');
  }

  it('shows only for a sound with two channels', async () => {
    twoSided(1);
    const mono = row(card('c2'), 'mic');
    await new Promise((r) => setTimeout(r, 20));
    expect(within(mono).queryByRole('switch', { name: 'Mic: centre' })).toBeNull();
  });

  it('plays the side that carries the sound in both ears, in every clip, in one step', async () => {
    twoSided(2);
    const mic = row(card('c2'), 'mic');
    const centre = await within(mic).findByRole('switch', { name: 'Mic: centre' });
    expect(centre).toHaveAttribute('aria-checked', 'false');
    expect(within(mic).getByText('Plays a sound recorded on one side in both ears, in every clip.')).toBeInTheDocument();
    await userEvent.click(centre);
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.centre');
    // r1's default and c2's own setting play from the right; c1's mic is on its own track and stays as it is; r2's
    // mic has no file this cache knows, so it keeps the side it plays from (the first).
    expect(ops).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'mic',
        patch: { mix: { volume: 1, muted: false, effects: [], centreFrom: 1 } },
      },
      {
        type: 'setClipSourceMix',
        clipId: 'c2',
        sourceId: 'mic',
        mix: { volume: 0.6, muted: false, effects: [], centreFrom: 1 },
      },
      {
        type: 'updateRecordingSource',
        recordingId: 'r2',
        sourceId: 'mic',
        patch: { mix: { volume: 1, muted: false, effects: [], centreFrom: 0 } },
      },
    ]);
  });

  it('shows how this clip plays a centred sound, so it can be turned off, whatever the cache says', async () => {
    twoSided(1);
    const snap = twoTakesSnapshot();
    snap.project.recordings[0].sources[1].mix.centreFrom = 1;
    snap.project.timeline.main[1].sourceMix = { mic: { volume: 0.6, muted: false, effects: [], centreFrom: 1 } };
    const centre = within(row(card('c2', snap), 'mic')).getByRole('switch', { name: 'Mic: centre' });
    expect(centre).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(centre);
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.asRecorded');
    expect(ops.slice(0, 2)).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'mic',
        patch: { mix: { volume: 1, muted: false, effects: [] } },
      },
      { type: 'setClipSourceMix', clipId: 'c2', sourceId: 'mic', mix: { volume: 0.6, muted: false, effects: [] } },
    ]);
  });
});

describe("a camera sound's Fill gaps switch", () => {
  /** r1 with a camera whose file carries sound, kept muted as a camera's sound starts. */
  function withCameraSound(fillsGaps?: true): Snapshot {
    const snap = twoTakesSnapshot();
    const v1 = snap.project.assets[0];
    snap.project.assets.push({ ...v1, id: 'cam1', path: '/media/cam1', name: 'cam1.mp4', hasAudio: true });
    snap.project.recordings[0].sources.push({
      id: 'camera',
      role: 'camera',
      label: 'capture.source.camera',
      runs: [{ assetId: 'cam1', offsetUs: 0, durationUs: 60_000_000, driftPpm: 0 }],
      nudgeUs: 0,
      mix: { volume: 1, muted: true, effects: [], ...(fillsGaps && { fillsGaps }) },
    });
    return snap;
  }

  it('is named after its sound and fills the gaps, for the whole recording, in one step', async () => {
    const camera = row(card('c2', withCameraSound()), 'camera');
    const fill = within(camera).getByRole('switch', { name: 'Camera sound: fill gaps' });
    expect(fill).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(fill);
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.fillGaps');
    expect(ops).toEqual([
      {
        type: 'updateRecordingSource',
        recordingId: 'r1',
        sourceId: 'camera',
        patch: { mix: { volume: 1, muted: true, effects: [], fillsGaps: true } },
      },
    ]);
  });

  it('turned off, leaves the gaps silent again', async () => {
    const camera = row(card('c2', withCameraSound(true)), 'camera');
    const fill = within(camera).getByRole('switch', { name: 'Camera sound: fill gaps' });
    expect(fill).toHaveAttribute('aria-checked', 'true');
    await userEvent.click(fill);
    const { label } = await lastApply();
    expect(label).toBe('undo.source.noFillGaps');
  });
});
