import { describe, expect, it } from 'vitest';
import type { Layout } from '../../bindings/Layout';
import { layoutPatchFor } from '../../lib/geometry';
import { useProject } from '../../state/project';
import { fixtureSnapshot } from '../../test/fixtures';
import { gizmoTargets } from './gizmoTargets';

/*
 * A layer moved by its motion keys is picked on the canvas where the keys draw it at the playhead, and a move there sets
 * its position keys at the playhead, leaving its own box where it rests.
 */

const S = 1_000_000;

describe('the canvas on a layer with motion keys', () => {
  it('picks it where its keys draw it, and a move sets the keys at the playhead', () => {
    const snapshot = fixtureSnapshot();
    const title = snapshot.project.timeline.tracks[0].items[0];
    title.layout.motion = {
      ...title.layout.motion,
      keys: { x: [{ tUs: 0, v: 0 }, { tUs: 2 * S, v: 0.2 }], y: [], scale: [], rotation: [], opacity: [] },
    };
    useProject.setState({ snapshot, pending: 0 });
    // 1 s into the title (it starts at 1 s): 0.1 right of its own box.
    const target = gizmoTargets(snapshot.project, 2 * S, 'landscape').find((t) => t.id === 'title')!;
    expect(target.rect.x).toBeCloseTo(title.layout.rect.x + 0.1);
    const moved = { ...target.rect, x: target.rect.x + 0.05 };
    const [op] = target.ops((l) => layoutPatchFor(l, moved, 'landscape', 'landscape'));
    const layout = (op as unknown as { patch: { layout: Layout } }).patch.layout;
    expect(layout.rect).toEqual(title.layout.rect);
    expect(layout.motion?.keys?.x.map((k) => [k.tUs, Math.round(k.v * 1000) / 1000])).toEqual([
      [0, 0],
      [S, 0.15],
      [2 * S, 0.2],
    ]);
    expect(layout.motion?.enter).toEqual(title.layout.motion?.enter);
  });
});
