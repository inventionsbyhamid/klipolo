import { describe, expect, it } from 'vitest';
import type { Item } from '../../../bindings/Item';
import type { Project } from '../../../bindings/Project';
import { fixtureProject } from '../../../test/fixtures';
import { editOps, editRoom, heldTo } from './editRoom';

const S = 1_000_000;
const FRAME = 33_333;

/** "a" plays the recording 2 to 20 s, "b" broll 2 to 8 s (2 s spare each side), "c" the recording 30 to 40 s. */
function abc(): Project {
  const p = fixtureProject();
  const [a, b] = p.timeline.main;
  p.timeline.main = [
    { ...a, sourceInUs: 2 * S },
    { ...b, sourceInUs: 2 * S, sourceOutUs: 8 * S },
    { ...a, id: 'c', sourceInUs: 30 * S, sourceOutUs: 40 * S },
  ];
  p.timeline.tracks = [];
  return p;
}

const dissolve = (seconds: number) => ({
  effect: 'klipolo.builtin/transition.dissolve',
  durationUs: seconds * S,
  params: {},
});

/** The camera separated from "a": its file starts 0.2 s into the take, so at a's 2 s it plays 1.8 s in. */
function withCamera(p: Project, locked = false): Project {
  const camera: Item = {
    id: 'a-camera',
    startUs: 0,
    durationUs: 18 * S,
    attach: { clipId: 'a', source: { type: 'recording', id: 'rec' }, sourceUs: 2 * S },
    layout: {
      anchor: 'center',
      rect: { x: 0, y: 0, w: 1, h: 1 },
      rotation: 0,
      opacity: 1,
      radius: 0,
      overrides: {},
    },
    content: {
      type: 'video',
      assetId: 'camera',
      sourceInUs: 1_800_000,
      speed: 1,
      volume: 1,
      muted: true,
      tie: { recording: 'rec', source: 'camera' },
    },
    effects: [],
  };
  p.timeline.tracks = [
    { id: 'cam', kind: 'overlay', name: 'Overlay 1', hidden: false, muted: false, locked, items: [camera] },
  ];
  return p;
}

describe('the room a roll, slip or slide has', () => {
  it("a roll is held by the footage before the next clip's in point and by a frame of it", () => {
    expect(editRoom(abc(), 'roll', 0)).toEqual({
      min: -2 * S,
      max: 6 * S - FRAME,
      minBy: 'footage',
      maxBy: 'length',
    });
    expect(editRoom(abc(), 'roll', 2)).toBeNull();
  });

  it("a slip is held by its own media and a slide by its neighbours'", () => {
    expect(editRoom(abc(), 'slip', 1)).toEqual({ min: -2 * S, max: 2 * S, minBy: 'footage', maxBy: 'footage' });
    expect(editRoom(abc(), 'slide', 1)).toEqual({
      min: FRAME - 18 * S,
      max: 10 * S - FRAME,
      minBy: 'length',
      maxBy: 'length',
    });
    expect(editRoom(abc(), 'slide', 0)).toBeNull();
    expect(editRoom(abc(), 'slide', 2)).toBeNull();
  });

  it('a transition keeps the footage it blends', () => {
    const p = abc();
    p.timeline.main[0] = { ...p.timeline.main[0], transitionOut: dissolve(1) };
    // The dissolve takes 0.5 s of b's footage before its in point.
    expect(editRoom(p, 'roll', 0)).toMatchObject({ min: -1.5 * S, minBy: 'transition' });
    expect(editRoom(p, 'slip', 1)).toMatchObject({ min: -1.5 * S, minBy: 'transition', max: 2 * S });
  });

  it("a separated camera holds its clip's slip to its own file, and a locked track holds it still", () => {
    expect(editRoom(withCamera(abc()), 'slip', 0)).toMatchObject({ min: -1_800_000, minBy: 'footage' });
    expect(editRoom(withCamera(abc(), true), 'slip', 0)).toEqual({
      min: 0,
      max: 0,
      minBy: 'locked',
      maxBy: 'locked',
    });
  });

  it("a separated sound inside the next clip holds a roll before the footage under it would be hidden", () => {
    const p = abc();
    const camera = withCamera(abc()).timeline.tracks[0].items[0];
    // A sound of the take tied to b, 2 s into it (b starts at 18 s, its footage at 2 s): its moment is b's 4 s.
    const voice: Item = {
      ...camera,
      id: 'voice',
      startUs: 20 * S,
      durationUs: S,
      attach: { clipId: 'b', source: { type: 'asset', id: 'broll' }, sourceUs: 4 * S },
      content: {
        type: 'audio',
        assetId: 'mic',
        sourceInUs: 20 * S,
        volume: 1,
        fadeInUs: 0,
        fadeOutUs: 0,
        effect: null,
        tie: { recording: 'rec', source: 'mic' },
      },
    };
    p.timeline.tracks = [
      { id: 'mic', kind: 'audio', name: 'Audio 1', hidden: false, muted: false, locked: false, items: [voice] },
    ];
    expect(editRoom(p, 'roll', 0)).toMatchObject({ max: 2 * S, maxBy: 'separated' });
  });

  it('a freeze frame has no footage to slip', () => {
    const p = abc();
    p.timeline.main[1] = { ...p.timeline.main[1], freezeAtUs: 3 * S };
    expect(editRoom(p, 'slip', 1)).toEqual({ min: 0, max: 0, minBy: 'freeze', maxBy: 'freeze' });
  });

  it('an edit past its room stops at the end it reached, saying what held it', () => {
    const room = editRoom(abc(), 'slip', 1)!;
    expect(heldTo(room, 3 * S)).toEqual({ by: 2 * S, held: 'footage' });
    expect(heldTo(room, S)).toEqual({ by: S, held: null });
    expect(editOps('slip', 'b', 0)).toEqual([]);
    expect(editOps('roll', 'a', FRAME)).toEqual([{ type: 'roll', clipId: 'a', byUs: FRAME }]);
  });
});
