import { useEffect, useState, type CSSProperties } from 'react';
import type { TextStyle } from '../bindings/TextStyle';
import { fonts } from '../bridge/fonts';

/** What a text's face is: its family, weight and style. */
export type FaceKey = Pick<TextStyle, 'font' | 'weight' | 'italic'>;

/** The family a bundled font is registered under in the page (never a family the system has). */
export const pageFamily = (family: string) => `kf:${family}`;

const keyOf = (f: FaceKey) => `${f.font.family}|${f.weight}|${f.italic ? 'i' : 'n'}`;
const loading = new Map<string, Promise<boolean>>();
const listeners = new Set<() => void>();

/**
 * The bundled face of `face` in the page: its bytes from Rust (`fonts_face`, the file the engine draws with), added as
 * a `FontFace` at the weight and style asked, so CSS matches it exactly. Once per face; resolves true once it is there
 * (false where the page has no `FontFace`, as in tests, or the font isn't one of Klipolo's).
 */
export function ensureFace(face: FaceKey): Promise<boolean> {
  const key = keyOf(face);
  let done = loading.get(key);
  if (done) return done;
  done =
    typeof FontFace === 'undefined' || typeof document === 'undefined' || !document.fonts
      ? Promise.resolve(false)
      : fonts
          .face(face.font.family, face.weight, face.italic)
          .then(async (bytes) => {
            const loaded = await new FontFace(pageFamily(face.font.family), bytes, {
              weight: String(face.weight),
              style: face.italic ? 'italic' : 'normal',
            }).load();
            document.fonts.add(loaded);
            for (const heard of listeners) heard();
            return true;
          })
          .catch(() => false);
  loading.set(key, done);
  return done;
}

/** Hears every face that joins the page (text measured in it is measured again). */
export function onFaceAdded(heard: () => void): () => void {
  listeners.add(heard);
  return () => listeners.delete(heard);
}

/** CSS for text in `face`: its bundled face, then Inter (the engine's stand-in), then the system's sans-serif. */
export function faceCss(face: FaceKey): CSSProperties {
  return {
    fontFamily: `"${pageFamily(face.font.family)}", "${pageFamily('Inter')}", sans-serif`,
    fontWeight: face.weight,
    fontStyle: face.italic ? 'italic' : 'normal',
    fontSynthesis: 'none',
  };
}

/** Loads `face` and re-renders once it is in the page (its first frame draws in the stand-in). */
export function useFace(face: FaceKey): void {
  const key = keyOf(face);
  const [, setLoaded] = useState('');
  useEffect(() => {
    let live = true;
    void ensureFace(face).then((ok) => {
      if (live && ok) setLoaded(key);
    });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the face's key says it all
  }, [key]);
}
