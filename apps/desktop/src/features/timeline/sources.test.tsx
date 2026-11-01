import { act, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../bindings/Project';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import { separated, twoTakesSnapshot } from '../../test/sounds';
import parity from './keys/source-mix-parity.json';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn(), undo: vi.fn() }, mediaUrl: (p: string) => p }));
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
const { useCommands } = await import('../command-palette/useCommands');
const { labelText } = await import('../../lib/labels');

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

describe('a separated sound', () => {
  it("sits under the sound lanes, named after its take's sound, drawn in the clip's blue with its waveform", () => {
    const snap = twoTakesSnapshot();
    // A plain sound effect on the same lane, for comparison.
    snap.project.timeline.tracks[0].items.push(separated('whoosh', 's1', 30, 31));
    show(snap);
    const sounds = screen.getByRole('group', { name: 'Sound effects' });
    const mic = sounds.querySelector<HTMLElement>('[data-block-id="c1-mic"]')!;
    expect(mic.getAttribute('aria-label')).toMatch(/^Sound layer, Mic of Recording 1, /);
    expect(mic).toHaveClass('is-recorded');
    expect(mic.querySelector('canvas')).not.toBeNull();
    const whoosh = sounds.querySelector<HTMLElement>('[data-block-id="whoosh"]')!;
    expect(whoosh).not.toHaveClass('is-recorded');
    expect(whoosh.querySelector('canvas')).toBeNull();
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

describe("a recording clip's menu", () => {
  /** Opens the menu of the clip named so. */
  const menuOf = (name: RegExp) => fireEvent.contextMenu(screen.getByRole('button', { name }));

  it('lists each recorded sound: its mute in this clip, then Separate', async () => {
    show(twoTakesSnapshot());
    menuOf(/^Recording 1, clip 2 of 3/);
    expect(await screen.findByRole('menuitem', { name: 'Mute Mic in this clip' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Mute Computer sound in this clip' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Separate Mic' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Separate Computer sound' })).toBeInTheDocument();
    // The screen is the main footage: it has neither.
    expect(screen.queryByRole('menuitem', { name: /Screen/ })).toBeNull();
  });

  it("mutes one sound in this clip alone, as the person's mute", async () => {
    show(twoTakesSnapshot());
    menuOf(/^Recording 1, clip 2 of 3/);
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Mute Mic in this clip' }));
    expect((await lastApply()).ops).toEqual([
      {
        type: 'setClipSourceMix',
        clipId: 'c2',
        sourceId: 'mic',
        mix: { volume: 0.6, muted: true, effects: [], mutedByPerson: true },
      },
    ]);
  });

  it('Separate makes the sound its own layer, in one step that Undo takes back', async () => {
    vi.mocked(bridge.undo)
      .mockReset()
      .mockImplementation(async () => {
        const s = useProject.getState().snapshot!;
        return {
          baseRevision: s.history.revision,
          history: { ...s.history, revision: s.history.revision + 1 },
          patch: { clips: [], items: [] },
        };
      });
    show(twoTakesSnapshot());
    menuOf(/^Recording 2, clip 3 of 3/);
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Separate Mic' }));
    expect(await lastApply()).toEqual({
      label: 'undo.source.separate',
      params: { source: { code: 'source.role.mic', params: {} } },
      ops: [{ type: 'detachSource', clipId: 'c3', sourceId: 'mic' }],
    });
    expect(labelText({ id: 'undo.source.separate', params: { source: { code: 'source.role.mic', params: {} } } })).toBe(
      'Separate Mic',
    );
    await act(() => useProject.getState().undo());
    expect(bridge.undo).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });

  it('keeps a separated sound as it is in its clip, and says why', async () => {
    show(twoTakesSnapshot());
    menuOf(/^Recording 1, clip 1 of 3/);
    const mute = await screen.findByRole('menuitem', { name: /^Unmute Mic in this clip/ });
    expect(mute).toHaveAttribute('aria-disabled', 'true');
    expect(mute).toHaveTextContent('Mic is on its own track here.');
    expect(screen.getByRole('menuitem', { name: /^Separate Mic/ })).toHaveAttribute('aria-disabled', 'true');
  });

  it('Separate is off on a sped-up clip, which says why', async () => {
    const snap = twoTakesSnapshot();
    snap.project.timeline.main[2].speed = 2;
    show(snap);
    menuOf(/^Recording 2, clip 3 of 3/);
    const item = await screen.findByRole('menuitem', { name: /^Separate Mic/ });
    expect(item).toHaveAttribute('aria-disabled', 'true');
    expect(item).toHaveTextContent('Set the clip back to normal speed before separating its sound.');
  });

  it("lists Separate camera with the sounds' Separate, in one group after their mutes", async () => {
    show(fixtureSnapshot());
    menuOf(/^Recording 1, clip 1 of 2/);
    const menu = await screen.findByRole('menu');
    await screen.findByRole('menuitem', { name: /^Separate camera/ });
    const rows = [...menu.querySelectorAll('[role="menuitem"], [role="separator"]')].map((el) =>
      el.getAttribute('role') === 'separator' ? '|' : (el.textContent ?? '').replace(/[⌫⌘⇧S]+$/u, ''),
    );
    const from = rows.indexOf('Mute Mic in this clip');
    expect(rows.slice(from, from + 5)).toEqual(['Mute Mic in this clip', '|', 'Separate camera', 'Separate Mic', '|']);
  });

  it('an imported clip has none', async () => {
    show(fixtureSnapshot());
    menuOf(/^broll\.mp4, clip 2 of 2/);
    await screen.findByRole('menuitem', { name: /^Split at playhead/ });
    expect(screen.queryByRole('menuitem', { name: /in this clip|^Separate/ })).toBeNull();
  });
});

describe('the sound commands', () => {
  it("are the selected recording clip's, by stable ids, and none without one", () => {
    const snap = twoTakesSnapshot();
    useProject.setState({ snapshot: snap, pending: 0 });
    let ids: string[] = [];
    function Probe() {
      ids = useCommands()
        .map((c) => c.id)
        .filter((id) => id.startsWith('source-'));
      return null;
    }
    const { rerender } = renderWithProviders(<Probe />);
    expect(ids).toEqual([]);
    act(() => useSelection.setState({ selectedId: 'c3' }));
    rerender(<Probe />);
    expect(ids).toEqual(['source-mute:mic', 'source-separate:mic']);
  });
});

describe("the lanes and menus make the pinned source tools' ops", () => {
  /** A part of `source-mix-parity.json` (the ops chat's source tools compile to, `sourceMix.test.ts`) as a snapshot. */
  const golden = (part: { project: unknown }): Snapshot => ({
    project: structuredClone(part.project) as Project,
    history: { revision: 3, canUndo: true, canRedo: false, undoLabel: null, redoLabel: null },
  });
  /** The pinned ops for one of the file's requests. */
  const pinnedOps = (name: string, args: Record<string, unknown>) =>
    parity.cases.find(
      (c) =>
        c.call.name === name &&
        Object.entries(args).every(([key, value]) => (c.call.args as Record<string, unknown>)[key] === value),
    )!.ops;

  it("two takes show one lane per kind, and the Mic lane's Mute is the pinned mute my mic", async () => {
    show(golden(parity));
    expect(screen.getAllByRole('group', { name: 'Mic' })).toHaveLength(1);
    expect(screen.getAllByRole('group', { name: 'Game sound' })).toHaveLength(1);
    await userEvent.click(within(screen.getByRole('group', { name: 'Mic' })).getByRole('button', { name: 'Mute Mic' }));
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.mute');
    expect(ops).toEqual(pinnedOps('muteSource', { source: 'mic', muted: true }));
  });

  it('two mics show Mic and Mic 2 lanes, and each header mutes its own', async () => {
    // A lav as the take's second mic (lav-2.wav): its lane takes the file's name.
    show(golden(parity.twoMics));
    expect(screen.getAllByRole('group', { name: 'Mic' })).toHaveLength(1);
    const lav = screen.getByRole('group', { name: 'lav-2.wav' });
    await userEvent.click(within(lav).getByRole('button', { name: 'Mute lav-2.wav' }));
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.mute');
    expect(ops).toEqual(parity.twoMics.cases[1].ops);
    expect(parity.twoMics.cases[1].call.args.which).toBe('2');
  });

  it('a second mic Klipolo named is Mic 2, on its lane and in its clip menu', async () => {
    const named = golden(parity.twoMics);
    named.project.recordings[0].sources.find((s) => s.id === 'mic-2')!.label = 'capture.source.mic';
    show(named);
    const second = screen.getByRole('group', { name: 'Mic 2' });
    expect(within(second).getByRole('button', { name: 'Mute Mic 2' })).toBeInTheDocument();
    // Both lanes show the mic's icon: each header's controls are named after its lane.
    expect(within(second).getByRole('toolbar', { name: 'Mic 2' })).toBeInTheDocument();
    const first = screen.getByRole('group', { name: 'Mic' });
    expect(within(first).getByRole('toolbar', { name: 'Mic' })).toBeInTheDocument();
    fireEvent.contextMenu(screen.getByRole('button', { name: /^Recording 1, clip 1 of 2/ }));
    expect(await screen.findByRole('menuitem', { name: 'Mute Mic in this clip' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Mute Mic 2 in this clip' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: /^Separate Mic 2/ })).toBeInTheDocument();
  });

  it('the Camera sound lane is muted by default and unmutes as the pinned ops do', async () => {
    show(golden(parity.cameraSound));
    // One lane for both interviews' cameras; the take, whose camera file has no sound, adds none.
    const lane = screen.getByRole('group', { name: 'Camera sound' });
    await userEvent.click(within(lane).getByRole('button', { name: 'Unmute Camera sound' }));
    const { label, ops } = await lastApply();
    expect(label).toBe('undo.source.unmute');
    const pinned = parity.cameraSound.cases.find(
      (c) => c.call.name === 'muteSource' && (c.call.args as { muted?: boolean }).muted === false,
    )!;
    expect(ops).toEqual(pinned.ops);
  });

  it("a camera's sound has a mute in this clip, never Separate, and Mute by Unmute while it fills gaps", async () => {
    show(golden(parity.cameraSound));
    fireEvent.contextMenu(screen.getByRole('button', { name: /^Recording 1, clip 1 of 4/ }));
    expect(await screen.findByRole('menuitem', { name: 'Unmute Camera sound in this clip' })).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Mute Camera sound in this clip (the gaps too)' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: /^Separate Camera sound/ })).toBeNull();
    expect(screen.getByRole('menuitem', { name: /^Separate ZOOM0001\.WAV/ })).toBeInTheDocument();
  });

  it("a camera's sound muted by the person has no second Mute", async () => {
    show(golden(parity.cameraSound.fillMuted));
    fireEvent.contextMenu(screen.getByRole('button', { name: /^Recording 1, clip 1 of 4/ }));
    expect(await screen.findByRole('menuitem', { name: 'Unmute Camera sound in this clip' })).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Mute Camera sound in this clip (the gaps too)' })).toBeNull();
  });
});
