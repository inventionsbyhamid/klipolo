import { describe, expect, it } from 'vitest';
import { searchLibrary, type Searchable } from './search';

/*
 * The library's search: by name, tag and kind, and by related words ("beach" finds the ocean, "boom" an impact sound),
 * every meaningful word asked for counting, exact words ranked first, and nothing found that wasn't asked for.
 */

const item = (name: string, category: string, tags: string[] = [], collection = 'Yours'): Searchable => ({
  name,
  category,
  kind: category === 'music' || category === 'sound' ? 'audio' : category === 'video' ? 'video' : 'image',
  tags,
  collection,
});
const search = (items: Searchable[], query: string) => searchLibrary(items, query, (i) => i);

describe('the library search', () => {
  it('finds available related labels without requiring tags', () => {
    const sea = item('Ocean blue.jpg', 'photo');
    expect(search([sea, item('City.jpg', 'photo')], 'find me beach photos')).toEqual([sea]);
  });

  it('requires each meaningful concept and ranks exact words first', () => {
    const exact = item('Calm guitar', 'music');
    const related = item('Peaceful guitar', 'music');
    expect(search([related, exact, item('Calm skyline', 'photo')], 'calm music')).toEqual([exact, related]);
  });

  it('ranks useful related files for conversational requests without contradicting explicit colors', () => {
    const warm = item('Warm glow', 'background');
    const gentle = item('Gentle gradient', 'background');
    expect(search([warm], 'something warm for my birthday')).toEqual([warm]);
    expect(search([gentle], 'a gentle background for a family celebration')).toEqual([gentle]);
    expect(search([item('Blue ocean', 'photo')], 'red ocean')).toEqual([]);
  });

  it('never invents results and preserves input order for empty queries', () => {
    const rows = [item('Skyline', 'photo')];
    expect(search(rows, 'spaceship')).toEqual([]);
    expect(search(rows, '')).toBe(rows);
    expect(search(rows, '  the  ')).toBe(rows);
  });

  it('finds the pack’s items by their tags, a name over a tag', () => {
    const whoosh = item('Whoosh', 'sound', ['transition', 'topic change', 'fast', 'air', 'swipe'], 'Built in');
    const swoosh = item('Swoosh', 'sound', ['transition', 'quick', 'swipe', 'air', 'reveal'], 'Built in');
    const mine = item('swipe.wav', 'sound');
    expect(search([whoosh, swoosh], 'topic change')).toEqual([whoosh]);
    expect(search([whoosh, swoosh, mine], 'swipe')).toEqual([mine, whoosh, swoosh]);
    // Plurals and case don't count.
    expect(search([whoosh, swoosh], 'Transitions')).toEqual([whoosh, swoosh]);
  });

  it('understands related words: "boom" finds an impact sound and "arrow" pointer stickers', () => {
    const impact = item('Impact', 'sound', ['impact', 'hit', 'heavy'], 'Built in');
    const ding = item('Ding', 'sound', ['bell', 'notification'], 'Built in');
    expect(search([ding, impact], 'boom')).toEqual([impact]);
    expect(search([ding, impact], 'a boom sound')).toEqual([impact]);
    const pointer = item('Hand pointer', 'sticker', ['pointer', 'hand', 'click'], 'Built in');
    const heart = item('Heart', 'sticker', ['heart', 'love'], 'Built in');
    expect(search([heart, pointer], 'arrow')).toEqual([pointer]);
    expect(search([heart, pointer], 'arrows')).toEqual([pointer]);
  });

  it('a kind asked for leaves out other kinds, and ranks a related one after its own', () => {
    const sticker = item('Star', 'sticker', ['star', 'gold'], 'Built in');
    const picture = item('Gold star.png', 'photo');
    expect(search([sticker, picture], 'gold sticker')).toEqual([sticker]);
    // A sticker is a picture too.
    expect(search([sticker, picture], 'star photo')).toEqual([picture, sticker]);
    expect(search([sticker, item('Kiln door.wav', 'sound')], 'star sound')).toEqual([]);
  });

  it('"favorites" asks for the starred items, not the star sticker', () => {
    expect(search([item('Star', 'sticker', ['star', 'gold'], 'Built in')], 'favorites')).toEqual([]);
  });
});
