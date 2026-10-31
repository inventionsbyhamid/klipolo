import type { CaptionPagesQuery } from '../bindings/CaptionPagesQuery';
import type { CaptionPagesView } from '../bindings/CaptionPagesView';
import type { FormatKind } from '../bindings/FormatKind';
import type { WordView } from '../bindings/WordView';
import { call } from './call';

/**
 * The open project's caption pages (`crates/klipolo-app/src/commands/captions.rs`), the engine's own: a window of one
 * format's, at most 200 at a time. Null when `projectId` isn't the open project.
 */
export const captions = {
  pages: (projectId: string, format: FormatKind, query: CaptionPagesQuery) =>
    call<CaptionPagesView | null>('captions_pages', { projectId, format, query }),
  /** Every place `format`'s captions show `text` (whole, ignoring case and marks), once per file word: "Fix all". */
  find: (projectId: string, format: FormatKind, text: string) =>
    call<WordView[] | null>('captions_find', { projectId, format, text }),
};
