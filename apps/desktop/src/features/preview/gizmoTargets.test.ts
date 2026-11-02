import { describe, expect, it } from 'vitest';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { gizmoTargets } from './gizmoTargets';

const FULL = { x: 0, y: 0, w: 1, h: 1 };

describe('a title template on the canvas', () => {
  it('the kinetic title is picked at its band of words near the top, not the whole frame', () => {
    const project = fixtureSnapshot().project;
    project.timeline.tracks[0].items = [
      {
        ...titleItem,
        id: 'hook',
        layout: { ...titleItem.layout, anchor: 'topLeft', rect: FULL, overrides: {}, motion: undefined },
        content: { type: 'generator', source: 'klipolo.builtin/scene.kinetic-title', params: { title: 'Day 1' } },
      },
    ];
    const hook = gizmoTargets(project, 2_000_000, 'landscape').find((t) => t.id === 'hook');
    // As its scene.json places it: 0.13 to 0.87 across, from 0.2 down, 220 of 1080p's pixels tall.
    expect(hook?.box).toEqual({ x: 0.13, y: 0.2, w: 0.74, h: 220 / 1080 });
    expect(hook?.movable).toBe(false);
  });
});
