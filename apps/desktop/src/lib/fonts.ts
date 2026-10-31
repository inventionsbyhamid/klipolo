import { useEffect, useState, type CSSProperties } from 'react';
import { create } from 'zustand';
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
 * a `FontFace` at the weight and style asked, so CSS matches it exactly. Once per face once it is there; resolves true
 * then (false where the page has no `FontFace`, as in tests, or the face couldn't be had: a later ask tries again).
 */
export function ensureFace(face: FaceKey): Promise<boolean> {
  if (typeof FontFace === 'undefined' || typeof document === 'undefined' || !document.fonts)
    return Promise.resolve(false);
  const key = keyOf(face);
  let done = loading.get(key);
  if (done) return done;
  done = fonts
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
    .catch(() => {
      // Not kept as failed: the next text in this face asks again.
      loading.delete(key);
      return false;
    });
  loading.set(key, done);
  return done;
}

/** The bundled families in the Font menu's order, from Rust (`fonts_bundled`); null until they are here. */
const useFamilies = create<{ families: string[] | null }>(() => ({ families: null }));
let familiesAsked = false;

function loadFamilies() {
  if (familiesAsked) return;
  familiesAsked = true;
  void fonts
    .bundled()
    .then((faces) => useFamilies.setState({ families: [...new Set(faces.map((f) => f.family))] }))
    .catch(() => {
      // Asked again by the next Font menu.
      familiesAsked = false;
    });
}

/** The families the Font menu offers: Rust's bundled ones, `fallback` until they are known. */
export function useBundledFamilies(fallback: string[]): string[] {
  const families = useFamilies((s) => s.families);
  useEffect(loadFamilies, []);
  return families && families.length > 0 ? families : fallback;
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
