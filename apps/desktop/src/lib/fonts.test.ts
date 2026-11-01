import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/*
 * The page's faces and the Font menu's families: a face that couldn't be had is asked for again later, never kept as
 * failed; a downloaded or installed font's face comes from the file the engine resolves, under a page family of its
 * own; a family whose fonts changed (a download) is asked for again; the menu lists Rust's families once they are in.
 */

vi.mock('../bridge/fonts', () => ({ fonts: { face: vi.fn(), faceOf: vi.fn(), preview: vi.fn(), bundled: vi.fn() } }));
const { fonts } = await import('../bridge/fonts');
const { ensureFace, faceCss, faceLoaded, forgetFamily, ensurePreview, previewFamily } = await import('./fonts');

const inter = { font: { family: 'Inter', source: { type: 'bundled' as const } }, weight: 700, italic: false };
const lato = { font: { family: 'Lato', source: { type: 'google' as const, version: 'v24' } }, weight: 400, italic: true };

let added: { family: string }[] = [];
let deleted: { family: string }[] = [];
beforeEach(() => {
  added = [];
  deleted = [];
  vi.stubGlobal(
    'FontFace',
    class {
      constructor(
        public family: string,
        public bytes: unknown,
        public descriptors: unknown,
      ) {}
      load() {
        return Promise.resolve(this);
      }
    },
  );
  Object.defineProperty(document, 'fonts', {
    configurable: true,
    value: { add: (f: { family: string }) => added.push(f), delete: (f: { family: string }) => deleted.push(f) },
  });
});
afterEach(() => vi.unstubAllGlobals());

describe('the page’s faces', () => {
  it('a face that failed to load is asked for again, and kept once it is there', async () => {
    vi.mocked(fonts.face).mockRejectedValueOnce(new Error('busy')).mockResolvedValue(new ArrayBuffer(8));
    expect(await ensureFace(inter)).toBe(false);
    expect(await ensureFace(inter)).toBe(true);
    expect(await ensureFace(inter)).toBe(true);
    expect(fonts.face).toHaveBeenCalledTimes(2);
    expect(added).toHaveLength(1);
  });

  it('a downloaded font’s face is the engine’s file, under a page family of its own, asked again once its fonts change', async () => {
    vi.mocked(fonts.faceOf).mockResolvedValue(new ArrayBuffer(8));
    expect(faceLoaded(lato)).toBe(false);
    expect(await ensureFace(lato)).toBe(true);
    expect(fonts.faceOf).toHaveBeenCalledWith(lato.font, 400, true);
    expect(faceLoaded(lato)).toBe(true);
    expect(added.map((f) => f.family)).toEqual(['kf:g:Lato:v24']);
    expect(faceCss(lato).fontFamily).toBe('"kf:g:Lato:v24", "kf:Inter", sans-serif');
    forgetFamily('Lato');
    expect(deleted.map((f) => f.family)).toEqual(['kf:g:Lato:v24']);
    expect(faceLoaded(lato)).toBe(false);
    expect(await ensureFace(lato)).toBe(true);
    expect(fonts.faceOf).toHaveBeenCalledTimes(2);
  });

  it('a Google family’s preview is asked once, and one that failed is never asked again', async () => {
    vi.mocked(fonts.preview).mockResolvedValueOnce(new ArrayBuffer(8)).mockRejectedValueOnce(new Error('offline'));
    expect(await ensurePreview('Poppins')).toBe(true);
    expect(await ensurePreview('Poppins')).toBe(true);
    expect(added.map((f) => f.family)).toEqual([previewFamily('Poppins')]);
    expect(await ensurePreview('Lobster')).toBe(false);
    expect(await ensurePreview('Lobster')).toBe(false);
    expect(fonts.preview).toHaveBeenCalledTimes(2);
  });
});
