import { describe, expect, it } from 'vitest';
import type { ProjectPatch } from '../bindings/ProjectPatch';
import { fixtureProject } from '../test/fixtures';
import { applyPatch } from './patch';

const empty: ProjectPatch = { clips: [], items: [] };

describe('applyPatch', () => {
  it('keeps everything a patch leaves alone', () => {
    const p = fixtureProject();
    expect(applyPatch(p, empty)).toBe(p);
    const renamed = applyPatch(p, { ...empty, name: 'Demo' })!;
    expect(renamed.name).toBe('Demo');
    expect(renamed.timeline).toBe(p.timeline);
    expect(renamed.assets).toBe(p.assets);
  });

  it("copies the project's cursor style like the captions, leaving the timeline alone", () => {
    const p = fixtureProject();
    const cursor = { ...p.cursor, size: 2 };
    const next = applyPatch(p, { ...empty, cursor })!;
    expect(next.cursor).toBe(cursor);
    expect(next.timeline).toBe(p.timeline);
    expect(next.captions).toBe(p.captions);
  });

  it('replaces only the changed item and keeps its neighbours', () => {
    const p = fixtureProject();
    const track = p.timeline.tracks[0];
    const moved = { ...track.items[0], startUs: 2_000_000 };
    const next = applyPatch(p, { ...empty, items: [moved] })!;
    expect(next.timeline.tracks[0].items[0]).toBe(moved);
    expect(next.timeline.main).toBe(p.timeline.main);
    for (let t = 1; t < p.timeline.tracks.length; t++)
      expect(next.timeline.tracks[t]).toBe(p.timeline.tracks[t]);
  });

  it('reorders clips by id and reuses the clip objects', () => {
    const p = fixtureProject();
    const [a, b] = p.timeline.main;
    const next = applyPatch(p, { ...empty, main: [b.id, a.id] })!;
    expect(next.timeline.main).toEqual([b, a]);
    expect(next.timeline.main[0]).toBe(b);
  });

  it('moves an item between tracks from the track shapes', () => {
    const p = fixtureProject();
    const [first, second] = p.timeline.tracks;
    const item = first.items[0];
    const shapes = p.timeline.tracks.map((t) => ({
      id: t.id,
      kind: t.kind,
      name: t.name,
      hidden: t.hidden,
      muted: t.muted,
      locked: t.locked,
      items: t.items.map((i) => i.id),
    }));
    shapes[0].items = shapes[0].items.filter((id) => id !== item.id);
    shapes[1].items = [...shapes[1].items, item.id];
    const next = applyPatch(p, { ...empty, tracks: shapes })!;
    expect(next.timeline.tracks[0].items.map((i) => i.id)).not.toContain(item.id);
    expect(next.timeline.tracks[1].items.at(-1)).toBe(item);
    expect(second.items.every((i) => next.timeline.tracks[1].items.includes(i))).toBe(true);
  });

  it("keeps a track's items array when only its settings change", () => {
    const p = fixtureProject();
    const shapes = p.timeline.tracks.map(({ items, ...header }) => ({
      ...header,
      items: items.map((i) => i.id),
    }));
    shapes[0].name = 'Titles';
    const next = applyPatch(p, { ...empty, tracks: shapes })!;
    expect(next.timeline.tracks[0].name).toBe('Titles');
    expect(next.timeline.tracks[0].items).toBe(p.timeline.tracks[0].items);
    expect(next.timeline.tracks[1]).toBe(p.timeline.tracks[1]);
    expect(next.timeline.main).toBe(p.timeline.main);
  });

  it('a solo-only change updates the track, and solo off again matches a track without it', () => {
    const p = fixtureProject();
    const shapes = p.timeline.tracks.map(({ items, ...header }) => ({
      ...header,
      items: items.map((i) => i.id),
    }));
    shapes[1] = { ...shapes[1], solo: true };
    const soloed = applyPatch(p, { ...empty, tracks: shapes })!;
    expect(soloed.timeline.tracks[1].solo).toBe(true);
    expect(soloed.timeline.tracks[1].items).toBe(p.timeline.tracks[1].items);
    expect(soloed.timeline.tracks[0]).toBe(p.timeline.tracks[0]);
    shapes[1] = { ...shapes[1], solo: false };
    expect(applyPatch(soloed, { ...empty, tracks: shapes })!.timeline.tracks[1].solo).toBe(false);
    // Rust leaves `solo` out when it's off: the fixture's tracks (no `solo`) are unchanged by a shape saying false.
    const off = p.timeline.tracks.map(({ items, ...header }) => ({
      ...header,
      solo: false,
      items: items.map((i) => i.id),
    }));
    expect(applyPatch(p, { ...empty, tracks: off })!.timeline.tracks).toBe(p.timeline.tracks);
  });

  it('gives up on a patch naming a clip or item it lacks, so the caller resyncs', () => {
    const p = fixtureProject();
    expect(applyPatch(p, { ...empty, main: ['a', 'ghost'] })).toBeNull();
    const shapes = p.timeline.tracks.map(({ items, ...header }) => ({
      ...header,
      items: items.map((i) => i.id),
    }));
    shapes[0].items.push('ghost');
    expect(applyPatch(p, { ...empty, tracks: shapes })).toBeNull();
  });
});
