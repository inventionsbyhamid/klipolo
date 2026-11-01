import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { usToPx } = await import('./scale');
const { tickStep } = await import('./ruler/ticks');
const { landedToast } = await import('../../state/recorder');
const { useToasts } = await import('../../state/toasts');
const { takeImport } = await import('../../test/recorderBridge');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
});

describe('timeline', () => {
  it('scales time to pixels and picks readable ticks', () => {
    expect(usToPx(2_500_000, 40)).toBe(100);
    expect(tickStep(40)).toBe(2);
    expect(tickStep(400)).toBe(0.2);
    expect(tickStep(8)).toBe(10);
  });

  it('labels half-second ruler ticks with tenths', () => {
    useUi.setState({ pxPerSecond: 160 });
    renderWithProviders(<Timeline />);
    expect(screen.getByText('0:00.5')).toBeInTheDocument();
  });

  it('draws clips proportional to their duration', () => {
    renderWithProviders(<Timeline />);
    const clip = screen.getByRole('button', { name: /^Recording 1/ });
    expect(clip).toHaveStyle({ width: '800px', left: '0px' });
    expect(screen.getByRole('button', { name: /^broll\.mp4/ })).toHaveStyle({
      left: '800px',
      width: '400px',
    });
  });

  it('a_recording_clip_is_named_as_its_toast_names_it', () => {
    renderWithProviders(<Timeline />);
    // The first clip plays the project's first recording: its block says so, never its screen file's name.
    expect(screen.getByRole('button', { name: /^Recording 1/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /screen\.mp4/ })).toBeNull();
    // As the toast that said it landed.
    useToasts.setState({ toasts: [] });
    landedToast(takeImport(useProject.getState().snapshot!, 'a', { recordingId: 'rec' }));
    expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['Added Recording 1, 0:20.']);
  });

  it("selects and seeks when a layer is clicked, past a title's fade-in so the paused preview shows it", async () => {
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: /Set up billing/ }));
    expect(useSelection.getState().selectedId).toBe('title');
    // The title starts at 1 s and fades in for 0.3 s: at 1 s exactly it is fully transparent.
    expect(usePlayback.getState().playheadUs).toBe(1_300_000);
  });

  it('a kept clip narrower than a clip block draws stays under the clip after it: no labels over it', () => {
    // A 0.2 s clip between the two (8 px at 40 px/s), selected so it doesn't join a band: its box is 16 px.
    const snapshot = fixtureSnapshot();
    const [a, b] = snapshot.project.timeline.main;
    snapshot.project.timeline.main = [a, { ...a, id: 'tiny', sourceOutUs: 200_000, camera: null }, b];
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'tiny' });
    renderWithProviders(<Timeline />);
    const tiny = document.querySelector<HTMLElement>('[data-block-id="tiny"]')!;
    const next = screen.getByRole('button', { name: /^broll\.mp4/ });
    expect(tiny.textContent).toBe('');
    expect(tiny).toHaveClass('isolate');
    // Later in the lane, so the next clip draws over (and takes the clicks in) the rest of the kept clip's box.
    expect(tiny.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(next.textContent).toContain('broll.mp4');
  });

  it('a kept layer narrower than a layer block draws stays under the layer after it: no label or handles over it', () => {
    // A 0.2 s layer (8 px at 40 px/s) ending 0.05 s before the title, selected so it doesn't join a band: its 12 px box
    // reaches 2 px into the title.
    const snapshot = fixtureSnapshot();
    const [overlay] = snapshot.project.timeline.tracks;
    const [title] = overlay.items;
    overlay.items = [{ ...title, id: 'tiny', startUs: 750_000, durationUs: 200_000, attach: null }, title];
    useProject.setState({ snapshot });
    useSelection.setState({ selectedId: 'tiny' });
    renderWithProviders(<Timeline />);
    const tiny = document.querySelector<HTMLElement>('[data-block-id="tiny"]')!;
    const next = document.querySelector<HTMLElement>('[data-block-id="title"]')!;
    expect(tiny.textContent).toBe('');
    expect(tiny.querySelector('[data-trim]')).toBeNull();
    expect(tiny).toHaveClass('isolate');
    expect(tiny.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(next.querySelectorAll('[data-trim]')).toHaveLength(2);
  });

  it('keeps the playhead when the clicked clip is already under it, so S splits there', async () => {
    usePlayback.setState({ playheadUs: 5_000_000 });
    renderWithProviders(<Timeline />);
    await userEvent.click(screen.getByRole('button', { name: /^Recording 1/ }));
    expect(useSelection.getState().selectedId).toBe('a');
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
  });

  it('counts layers that don’t fit in a "+N" that selects one, even when they began left of the view', async () => {
    // Four 10-minute layers from 0:00: three rows, and one layer over. The view is scrolled to 5:00.
    const project = longProject();
    const [first] = project.timeline.tracks;
    project.timeline.tracks = [0, 1, 2, 3].map((n) => ({
      ...first,
      id: `o${n}`,
      items: [{ ...first.items[0], id: `l${n}`, startUs: 0, durationUs: 600_000_000 }],
    }));
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project }, pending: 0 });
    renderWithProviders(<Timeline />);
    const scroller = screen.getByTestId('ruler').closest<HTMLElement>('.overflow-auto')!;
    act(() => {
      scroller.scrollLeft = 300 * 40;
      scroller.dispatchEvent(new Event('scroll'));
    });
    // Its name starts with what it shows ("+1"), for voice control.
    await userEvent.click(screen.getByRole('button', { name: '+1 more layer, select it' }));
    // The bottom track's layer is the one packed away (the top tracks take the rows first).
    expect(useSelection.getState().selectedId).toBe('l0');
  });

  it('reaches the end of a layer past the main track: the ruler runs to it and a click there seeks there', () => {
    // The 30 s fixture with its title moved to 40 to 50 s, after a gap: the video is 50 s long. A 60 s song doesn't
    // count.
    const snapshot = fixtureSnapshot();
    const [titles, sound] = snapshot.project.timeline.tracks;
    titles.items = [{ ...titles.items[0], startUs: 40_000_000, durationUs: 10_000_000 }];
    sound.items = [{ ...sound.items[0], durationUs: 60_000_000 }];
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    const ruler = screen.getByTestId('ruler');
    // 50 s and the 10 s tail at 40 px/s.
    expect(ruler).toHaveStyle({ width: `${usToPx(60_000_000, 40)}px` });
    fireEvent.pointerDown(ruler, { button: 0, clientX: usToPx(45_000_000, 40), pointerId: 1 });
    expect(usePlayback.getState().playheadUs).toBe(45_000_000);
    // Past the last layer it stops at the video's end, not at the main track's.
    fireEvent.pointerDown(ruler, { button: 0, clientX: usToPx(58_000_000, 40), pointerId: 1 });
    expect(usePlayback.getState().playheadUs).toBe(50_000_000);
  });
});
