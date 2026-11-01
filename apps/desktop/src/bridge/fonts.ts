import { invoke } from '@tauri-apps/api/core';
import type { Browse } from '../bindings/Browse';
import type { BrowseAsk } from '../bindings/BrowseAsk';
import type { BundledFont } from '../bindings/BundledFont';
import type { CatalogueInfo } from '../bindings/CatalogueInfo';
import type { CjkFonts } from '../bindings/CjkFonts';
import type { CjkScript } from '../bindings/CjkScript';
import type { DownloadedFamily } from '../bindings/DownloadedFamily';
import type { DownloadedId } from '../bindings/DownloadedId';
import type { FontRef } from '../bindings/FontRef';
import type { FontRow } from '../bindings/FontRow';
import type { GoogleFonts } from '../bindings/GoogleFonts';
import type { MissingAnswer } from '../bindings/MissingAnswer';
import type { SearchFilter } from '../bindings/SearchFilter';
import { call } from './call';

/**
 * Klipolo's fonts (`crates/klipolo-app/src/commands/fonts.rs`): the 12 bundled families, the Font menu's search and
 * browse over them, this computer's and Google's, the faces' bytes, and Google Fonts downloads, each refused by Rust
 * before the person's one consent or while Settings' switch is off.
 */
export const fonts = {
  /** Every face of the families the Font menu offers, in its order. */
  bundled: () => call<BundledFont[]>('fonts_bundled'),
  /** The bytes of `family`'s bundled face nearest `weight` and `italic`: the very file the engine draws the text with. */
  face: (family: string, weight: number, italic: boolean) =>
    invoke<ArrayBuffer>('fonts_face', { family, weight, italic }),
  /** The bytes of the face `font` draws with (downloaded or this computer's), as the engine resolves it. */
  faceOf: (font: FontRef, weight: number, italic: boolean) =>
    invoke<ArrayBuffer>('fonts_face_of', { font, weight, italic }),
  /** The families matching `query`, best first: the bundled set, this computer's, Google's. */
  search: (query: string, filter: SearchFilter) => call<FontRow[]>('fonts_search', { query, filter }),
  /** The menu's browse (an empty query): its sections' counts; the rows come by window. */
  browse: (ask: BrowseAsk) => call<Browse>('fonts_browse', { ask }),
  /** The browse's rows `from..to`. */
  browseRows: (ask: BrowseAsk, from: number, to: number) => call<FontRow[]>('fonts_browse_rows', { ask, from, to }),
  /** The row of exactly `font` (else a family of its name), for the menu's button and its Weight menu. */
  row: (font: FontRef, weight: number, italic: boolean) => call<FontRow | null>('fonts_row', { font, weight, italic }),
  /** Google Fonts' switch and the person's one consent. */
  google: () => call<GoogleFonts>('fonts_google'),
  /** Settings' switch. */
  setGoogle: (enabled: boolean) => call<GoogleFonts>('fonts_set_google', { enabled }),
  /** "Download from Google Fonts?" confirmed. */
  consent: () => call<GoogleFonts>('fonts_consent'),
  /** Downloads `family` for a text at `weight` and `italic` (`version`: a project's own, else the catalogue's). */
  download: (family: string, version: string | null, weight: number, italic: boolean) =>
    call<FontRef>('fonts_download', { family, version, weight, italic }),
  cancel: (family: string) => call<void>('fonts_cancel', { family }),
  /** The online preview of a Google family's name (after the consent): a small font file. */
  preview: (family: string) => invoke<ArrayBuffer>('fonts_preview', { family }),
  /** The downloaded families, with their sizes (Settings). */
  downloaded: () => call<DownloadedFamily[]>('fonts_downloaded'),
  /** Removes a downloaded family by its id; refused while an export runs. */
  remove: (id: DownloadedId) => call<void>('fonts_remove', { id }),
  /** Shows the downloaded fonts' folder in Finder. */
  reveal: () => call<void>('fonts_reveal'),
  /** Which Google Fonts list the app ships: its date, or that it is a test build's sample. */
  catalogue: () => call<CatalogueInfo>('fonts_catalogue'),
  /** The fonts the open project `projectId` uses that aren't here, once this Mac's are read (else `pending`). */
  missing: (projectId: string) => call<MissingAnswer>('fonts_missing', { projectId }),
  /** Noto Sans SC, TC, JP and KR: their faces' sizes, and which are here. */
  cjk: () => call<CjkFonts[]>('fonts_cjk'),
  /** The Noto Sans of `script` at `weight` (Chinese, Japanese or Korean text's fallback). */
  downloadCjk: (script: CjkScript, weight: number) => call<FontRef>('fonts_cjk_download', { script, weight }),
};
