import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { EXTENSION_KINDS, kindOfPath } from './mediaKinds';

/*
 * The page's file kinds by extension follow the import's own lists (`crates/klipolo-app/src/media.rs`): every type it
 * imports has a kind here, a picture exactly when it is one of its pictures, and nothing else has one.
 */

const media = readFileSync(resolve(__dirname, '../../../../crates/klipolo-app/src/media.rs'), 'utf8');

/** The quoted words of the Rust list `name`. */
function rustList(name: string): string[] {
  const body = new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(media);
  if (!body) throw new Error(`${name} isn't in media.rs`);
  return [...body[1].matchAll(/"([a-z0-9]+)"/g)].map((m) => m[1]);
}

describe('file kinds by extension', () => {
  it('match the extensions Rust imports and the ones it reads as pictures', () => {
    const imported = rustList('MEDIA_EXTENSIONS');
    const pictures = rustList('PICTURE_EXTENSIONS');
    expect(Object.keys(EXTENSION_KINDS).sort()).toEqual([...imported].sort());
    for (const ext of imported) {
      const kind = kindOfPath(`/m/file.${ext}`);
      expect(kind, ext).not.toBeNull();
      expect(kind === 'image', ext).toBe(pictures.includes(ext));
    }
    for (const ext of ['svg', 'txt', 'pdf', 'heif', 'mpg']) expect(kindOfPath(`/m/file.${ext}`), ext).toBeNull();
    expect(kindOfPath('/m/Clip.MOV')).toBe('video');
    expect(kindOfPath('/m.d/noext')).toBeNull();
  });
});
