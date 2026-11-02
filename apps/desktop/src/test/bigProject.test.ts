import { describe, expect, it } from 'vitest';
import { shortProject, talkPages, talkWords, TWO_HOURS_US, twoHourProject } from './bigProject';

const S = 1_000_000;
const mainUs = (p: ReturnType<typeof twoHourProject>) =>
  p.timeline.main.reduce((sum, c) => sum + (c.sourceOutUs - c.sourceInUs) / c.speed, 0);
const layers = (p: ReturnType<typeof twoHourProject>) => p.timeline.tracks.flatMap((t) => t.items);

describe('the big perf projects', () => {
  it('the 2-hour talk: 300 pieces, 200 layers with 4 video layers, 120 markers, Word pop captions', () => {
    const p = twoHourProject('/perf');
    expect(p.timeline.main).toHaveLength(300);
    expect(mainUs(p)).toBe(TWO_HOURS_US);
    expect(layers(p)).toHaveLength(200);
    expect(layers(p).filter((i) => i.content.type === 'video')).toHaveLength(4);
    expect(layers(p).filter((i) => i.content.type === 'audio' && i.content.volumeKeys?.length)).toHaveLength(30);
    expect(layers(p).filter((i) => i.layout.motion?.keys).length).toBeGreaterThanOrEqual(60);
    expect(p.recordings[0].markers).toHaveLength(120);
    expect(p.captions).toMatchObject({ enabled: true, style: 'klipolo.builtin/caption.word-pop' });
    expect(p.assets.find((a) => a.id === 'talk')!.path).toBe('/perf/talk-1080p-2h.mp4');
    // Each track's layers are in order and never overlap.
    for (const t of p.timeline.tracks)
      t.items.forEach((item, i) => i > 0 && expect(item.startUs).toBeGreaterThanOrEqual(t.items[i - 1].startUs + t.items[i - 1].durationUs));
  });

  it('the short: 60 s in 9:16 with captions, music, titles, sounds, stickers and zooms', () => {
    const p = shortProject();
    expect(p.settings.primaryFormat).toBe('portrait');
    expect(mainUs(p)).toBe(60 * S);
    expect(p.captions.enabled).toBe(true);
    expect(new Set(layers(p).map((i) => i.content.type))).toEqual(new Set(['zoom', 'image', 'text', 'audio']));
  });

  it('18,000 caption words over two hours, three to a page', () => {
    const words = talkWords();
    expect(words).toHaveLength(18_000);
    expect(words[words.length - 1].endUs).toBeLessThanOrEqual(TWO_HOURS_US);
    const pages = talkPages(words);
    expect(pages).toHaveLength(6000);
    expect(pages[5999].words).toHaveLength(3);
  });
});
