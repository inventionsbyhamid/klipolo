/*
 * The library's search: words typed find items by name, tag and kind, and by related words from a small, fixed list
 * ("beach" finds the ocean, "boom" an impact sound, "arrow" a pointer). It reads only the labels it is given, never
 * a file's content. Every meaningful word asked for counts; a name match ranks over a tag, a tag over a kind, and a
 * related word last. A color or a kind asked for that an item doesn't have leaves it out.
 */

/** What the search reads of a library item. */
export interface Searchable {
  name: string;
  tags: readonly string[];
  /** What it is: video, photo, background, sticker, music or sound. */
  category: string;
  /** How it plays: video, image or audio. */
  kind: string;
  /** Where it is from ("Built in", "Your sounds"). */
  collection: string;
}

/** Words that mean much the same here: any one of a group finds the others. */
const GROUPS = [
  ['sea', 'ocean', 'beach', 'coast', 'seaside'],
  ['forest', 'woods', 'woodland', 'trees'],
  ['city', 'urban', 'downtown', 'skyline'],
  ['mountain', 'mountains', 'alpine'],
  ['birthday', 'celebration', 'celebrate', 'party', 'festive'],
  ['wedding', 'romantic', 'romance'],
  ['family', 'together', 'home'],
  ['happy', 'cheerful', 'joyful', 'upbeat'],
  ['calm', 'relaxing', 'peaceful', 'ambient'],
  ['sad', 'melancholy', 'emotional'],
  ['cinematic', 'cinema', 'dramatic'],
  ['background', 'backdrop', 'wallpaper'],
  ['sticker', 'decal', 'emoji'],
  ['music', 'song', 'soundtrack', 'instrumental'],
  ['sound', 'sfx', 'effect'],
  ['video', 'footage', 'clip'],
  ['photo', 'photograph', 'picture', 'image'],
  ['night', 'nighttime', 'dark'],
  ['sunrise', 'dawn'],
  ['sunset', 'dusk'],
  ['fast', 'quick', 'energetic'],
  ['slow', 'gentle'],
  ['warm', 'golden'],
  ['cool', 'blue'],
  // Sound effects and stickers, as the pack tags them.
  ['boom', 'impact', 'hit', 'bang', 'thud', 'punch', 'crash', 'explosion'],
  ['arrow', 'pointer', 'point', 'cursor'],
  ['whoosh', 'swoosh', 'swipe', 'transition'],
  ['ding', 'bell', 'chime', 'notification'],
  ['click', 'tap', 'button'],
  ['applause', 'clap', 'cheer', 'crowd'],
  ['laugh', 'funny', 'comedy', 'joke'],
  ['riser', 'rise', 'build', 'tension'],
  ['sparkle', 'magic', 'shine', 'twinkle'],
  ['check', 'tick', 'done', 'correct'],
  ['heart', 'love', 'like'],
  ['star', 'favorite', 'favourite'],
  ['speech', 'talk', 'quote'],
  ['circle', 'ring', 'highlight'],
];
/** Words that ask for nothing. */
const IGNORED = new Set(['a', 'an', 'the', 'of', 'for', 'with', 'and', 'please', 'show', 'find', 'me', 'some', 'my', 'something']);
/** A color asked for that an item doesn't have leaves it out. */
const COLORS = new Set(['red', 'green', 'blue', 'purple', 'orange', 'black', 'white', 'pink']);
/** So does a kind of item. */
const KINDS = new Set(['music', 'sound', 'video', 'photo', 'background', 'sticker']);

/** `value`'s words, lowercase, without accents. */
const tokens = (value: string) =>
  value
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .match(/[\p{L}\p{N}]+/gu) ?? [];
/** A plural read as its one: "sounds" is "sound". */
const singular = (word: string) => (word.length > 3 && word.endsWith('s') ? word.slice(0, -1) : word);

/**
 * The items of `items` that `query` asks for, best first (in `items`' order between equals); `items` itself when the
 * query asks for nothing. `about` says what each item is called and tagged.
 */
export function searchLibrary<T>(items: T[], query: string, about: (item: T) => Searchable): T[] {
  const terms = [...new Set(tokens(query).filter((term) => !IGNORED.has(term)).map(singular))];
  if (!terms.length) return items;
  const queries = terms.map((term) => {
    const related = GROUPS.filter((group) => group.some((word) => singular(word) === term))
      .flat()
      .map(singular);
    return { term, alternatives: new Set([term, ...related]) };
  });
  return items
    .map((item, index) => {
      const said = about(item);
      const name = new Set(tokens(said.name).map(singular));
      const tags = new Set(said.tags.flatMap(tokens).map(singular));
      const category = new Set(tokens(`${said.category} ${said.kind} ${said.collection}`).map(singular));
      let score = 0;
      let matched = 0;
      let contentMatched = false;
      for (const { term, alternatives } of queries) {
        if (name.has(term)) score += 12;
        else if (tags.has(term)) score += 10;
        else if (category.has(term)) score += 6;
        else if ([...alternatives].some((word) => name.has(word) || tags.has(word) || category.has(word))) score += 3;
        else {
          if (COLORS.has(term) || KINDS.has(term)) return { item, index, score: 0 };
          continue;
        }
        matched++;
        if (!KINDS.has(term)) contentMatched = true;
      }
      // Only a kind matched ("sounds" with "boom sounds"): not what was asked for.
      if (!contentMatched && queries.some((q) => !KINDS.has(q.term))) score = 0;
      return { item, index, score: score * (matched / queries.length) };
    })
    .filter((row) => row.score > 0)
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map((row) => row.item);
}
