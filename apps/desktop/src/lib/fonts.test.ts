import { afterEach, describe, expect, it, vi } from 'vitest';

/*
 * The page's bundled faces and the Font menu's families: a face that couldn't be had is asked for again later, never
 * kept as failed; the menu lists Rust's families once they are in.
 */

vi.mock('../bridge/fonts', () => ({ fonts: { face: vi.fn(), bundled: vi.fn() } }));
const { fonts } = await import('../bridge/fonts');
const { ensureFace } = await import('./fonts');

const inter = { font: { family: 'Inter', source: { type: 'bundled' as const } }, weight: 700, italic: false };

afterEach(() => vi.unstubAllGlobals());

describe('bundled faces', () => {
  it('a face that failed to load is asked for again, and kept once it is there', async () => {
    const added: unknown[] = [];
    vi.stubGlobal(
      'FontFace',
      class {
        load() {
          return Promise.resolve(this);
        }
      },
    );
    Object.defineProperty(document, 'fonts', { configurable: true, value: { add: (f: unknown) => added.push(f) } });
    vi.mocked(fonts.face).mockRejectedValueOnce(new Error('busy')).mockResolvedValue(new ArrayBuffer(8));
    expect(await ensureFace(inter)).toBe(false);
    expect(await ensureFace(inter)).toBe(true);
    expect(await ensureFace(inter)).toBe(true);
    expect(fonts.face).toHaveBeenCalledTimes(2);
    expect(added).toHaveLength(1);
  });
});
