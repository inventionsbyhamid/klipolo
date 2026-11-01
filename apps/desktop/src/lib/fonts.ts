import { useEffect, useState, type CSSProperties } from 'react';
import { create } from 'zustand';
import type { FontRef } from '../bindings/FontRef';
import type { TextStyle } from '../bindings/TextStyle';
import { fonts } from '../bridge/fonts';

/** What a text's face is: its family, weight and style. */
export type FaceKey = Pick<TextStyle, 'font' | 'weight' | 'italic'>;

/** The family a bundled font is registered under in the page (never a family the system has). */
export const pageFamily = (family: string) => `kf:${family}`;

/**
 * The family a font's faces are registered under in the page: a bundled one by its name, a downloaded or installed
 * one by its source too, so a Google "Lato" and this computer's never stand in for each other.
 */
export function pageFamilyOf(font: FontRef): string {
  switch (font.source.type) {
    case 'bundled':
      return pageFamily(font.family);
    case 'google':
      return `kf:g:${font.family}:${font.source.version}`;
    case 'installed':
      return `kf:i:${font.family}`;
  }
}

const keyOf = (f: FaceKey) => `${pageFamilyOf(f.font)}|${f.weight}|${f.italic ? 'i' : 'n'}`;
/** Each face asked for, by key, with its family: what `forgetFamily` lets go. */
const loading = new Map<string, { family: string; installed: boolean; done: Promise<boolean> }>();
/** The faces in the page, by key. */
const loaded = new Map<string, FontFace>();
const listeners = new Set<() => void>();

/** Whether the page can draw in `face`. */
const pageHasFonts = () => typeof FontFace !== 'undefined' && typeof document !== 'undefined' && !!document.fonts;

/**
 * The face of `face` in the page: its bytes from Rust (the file the engine draws with: `fonts_face` for a bundled one,
 * `fonts_face_of` for a downloaded or installed one), added as a `FontFace` at the weight and style asked, so CSS
 * matches it exactly. Once per face once it is there; resolves true then (false where the page has no `FontFace`, as
 * in tests, or the face couldn't be had: a later ask tries again).
 */
export function ensureFace(face: FaceKey): Promise<boolean> {
  if (!pageHasFonts()) return Promise.resolve(false);
  const key = keyOf(face);
  const asked = loading.get(key);
  if (asked) return asked.done;
  const bytes =
    face.font.source.type === 'bundled'
      ? fonts.face(face.font.family, face.weight, face.italic)
      : fonts.faceOf(face.font, face.weight, face.italic);
  const done = bytes
    .then(async (data) => {
      const added = await new FontFace(pageFamilyOf(face.font), data, {
        weight: String(face.weight),
        style: face.italic ? 'italic' : 'normal',
      }).load();
      // Let go meanwhile (its fonts changed): the newer ask adds its own.
      if (loading.get(key)?.done !== done) return false;
      document.fonts.add(added);
      loaded.set(key, added);
      for (const heard of listeners) heard();
      return true;
    })
    .catch(() => {
      // Not kept as failed: the next text in this face asks again.
      if (loading.get(key)?.done === done) loading.delete(key);
      return false;
    });
  loading.set(key, { family: face.font.family, installed: face.font.source.type === 'installed', done });
  return done;
}

/** Whether `face` is in the page already: a row drawing it needs to wait for nothing. */
export function faceLoaded(face: FaceKey): boolean {
  return loaded.has(keyOf(face));
}

/**
 * `family`'s fonts changed (a download finished, a removal): its faces leave the page and are asked for again, so a
 * text drawn in a stand-in meanwhile is drawn in the font now.
 */
export function forgetFamily(family: string): void {
  forgetWhere((asked) => asked.family === family);
}

/**
 * This Mac's fonts arrived (the scan): every installed font's face is asked for again, as one asked before them was
 * answered with the stand-in.
 */
export function forgetInstalled(): void {
  forgetWhere((asked) => asked.installed);
}

function forgetWhere(gone: (asked: { family: string; installed: boolean }) => boolean): void {
  for (const [key, asked] of loading) {
    if (!gone(asked)) continue;
    loading.delete(key);
    const face = loaded.get(key);
    if (face) document.fonts.delete(face);
    loaded.delete(key);
  }
  for (const heard of listeners) heard();
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

/** Hears every face that joins or leaves the page (text measured in it is measured again). */
export function onFaceAdded(heard: () => void): () => void {
  listeners.add(heard);
  return () => listeners.delete(heard);
}

/** CSS for text in `face`: its own face, then Inter (the engine's stand-in), then the system's sans-serif. */
export function faceCss(face: FaceKey): CSSProperties {
  return {
    fontFamily: `"${pageFamilyOf(face.font)}", "${pageFamily('Inter')}", sans-serif`,
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

/** The page family of a Google family's online preview (its name only, in Google's small `menu` file). */
export const previewFamily = (family: string) => `kfp:${family}`;

/** Each preview asked for: true once it is in the page, false when it couldn't be had (never asked again). */
const previews = new Map<string, Promise<boolean>>();

/**
 * Google's preview of `family`'s name in the page (after the person's consent: Rust refuses it before), once. A
 * failure (offline, refused) is kept, so a list scrolled back and forth never asks again.
 */
export function ensurePreview(family: string): Promise<boolean> {
  if (!pageHasFonts()) return Promise.resolve(false);
  const asked = previews.get(family);
  if (asked) return asked;
  const done = fonts
    .preview(family)
    .then(async (data) => {
      document.fonts.add(await new FontFace(previewFamily(family), data).load());
      return true;
    })
    .catch(() => false);
  previews.set(family, done);
  return done;
}

/** `family` is downloaded now: its row draws in its own face, so the preview's is let go of. */
export function forgetPreview(family: string): void {
  previews.delete(family);
}
