import { describe, expect, it } from 'vitest';
import type { EffectInstance } from '../../bindings/EffectInstance';
import type { Project } from '../../bindings/Project';
import { TEXT_SWATCHES } from '../../lib/swatches';
import { fixtureSnapshot } from '../../test/fixtures';
import { projectColors, VIDEO_MAX } from './projectColors';

const effect = (params: EffectInstance['params']): EffectInstance => ({
  id: 'e',
  effect: 'klipolo.builtin/test',
  enabled: true,
  params,
});

/** The fixture with a copy of its title in these colours (the fixture's own title object is shared between tests). */
function withTitle(color: string, background: string | null, outline: string | null): Project {
  const { project } = fixtureSnapshot();
  const track = project.timeline.tracks[0];
  const [title] = track.items;
  if (title.content.type !== 'text') throw new Error('fixture title');
  const style = {
    ...title.content.style,
    color,
    background,
    outline: outline === null ? null : { color: outline, width: 2 },
  };
  track.items = [{ ...title, effects: [], content: { ...title.content, style } }];
  return project;
}

describe('projectColors', () => {
  it('lists the video’s own colours, most used first, never a preset', () => {
    const project = withTitle('#123456', '#FFD02E', null);
    project.captions = { ...project.captions, accent: '#abcdef' };
    project.wordLooks = [{ assetId: 'mic', startUs: 0, endUs: 1, look: { color: '#123456' } }];
    expect(
      projectColors(
        project,
        TEXT_SWATCHES.map((s) => s.value),
      ),
    ).toEqual(['#123456', '#ABCDEF']);
  });

  it('reads outlines and every effect colour, without the alpha: one colour whatever its case or opacity', () => {
    const project = withTitle('#FFFFFF', null, '#00ff0080');
    project.timeline.main[0].effects = [effect({ tint: '#00FF00', amount: 0.5, label: 'abc', on: true })];
    project.timeline.tracks[0].items[0].effects = [effect({ glow: '#AA0000CC', at: { x: 0, y: 0 } })];
    expect(projectColors(project, [])).toEqual(['#00FF00', '#FFFFFF', '#AA0000']);
  });

  it('at most eight; a tie keeps the order met; an excluded colour goes whatever its opacity', () => {
    const project = withTitle('#FFFFFF', null, null);
    const hexes = [
      '#010101',
      '#020202',
      '#030303',
      '#040404',
      '#050505',
      '#060606',
      '#070707',
      '#080808',
      '#090909',
    ];
    project.wordLooks = [...hexes, '#090909'].map((color, i) => ({
      assetId: 'mic',
      startUs: i,
      endUs: i + 1,
      look: { color },
    }));
    const found = projectColors(project, ['#ffffff', '#01010180']);
    expect(found).toHaveLength(VIDEO_MAX);
    expect(found).toEqual(['#090909', ...hexes.slice(1, 8)]);
  });

  it('counts the background’s colours only while it shows them', () => {
    const project = withTitle('#FFFFFF', null, null);
    // Every project has a background colour (#101114) under `kind: none`: nobody chose it.
    expect(projectColors(project, ['#FFFFFF'])).toEqual([]);
    project.background = { ...project.background, kind: 'gradient', colorEnd: '#00FFFF' };
    expect(projectColors(project, ['#FFFFFF'])).toEqual(['#101114', '#00FFFF']);
    project.background = { ...project.background, kind: 'solid' };
    expect(projectColors(project, ['#FFFFFF'])).toEqual(['#101114']);
  });
});

describe('projectColors and title templates', () => {
  it('counts a title template’s colour settings', () => {
    const project = withTitle('#FFFFFF', null, null);
    const [title] = project.timeline.tracks[0].items;
    project.timeline.tracks[0].items.push({
      ...title,
      id: 'card',
      content: { type: 'generator', source: 'klipolo.builtin/scene.lowerThird', params: { text: 'Hi', accent: '#336699' } },
    });
    expect(projectColors(project, ['#FFFFFF'])).toEqual(['#336699']);
  });
});
