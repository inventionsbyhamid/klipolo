/**
 * The block elements on screen, by id. Gestures move and size them directly (transforms, one element's width), so a
 * drag never re-renders React; `clearShifts` puts every touched block back once the new revision has drawn.
 */
const blocks = new Map<string, HTMLElement>();
const touched = new Set<string>();

/**
 * Records `el` as block `id` (a ref callback). The function it returns forgets it, but only while `el` is still the
 * one recorded: a block remounted elsewhere (another lane, compact ↔ expanded) may register before the old one's
 * cleanup runs.
 */
export function registerBlock(id: string, el: HTMLElement | null): () => void {
  if (el) blocks.set(id, el);
  return () => {
    if (el && blocks.get(id) === el) blocks.delete(id);
  };
}

export function blockElement(id: string): HTMLElement | null {
  return blocks.get(id) ?? null;
}

export function shiftBlock(id: string, dx: number, dy = 0) {
  const el = blocks.get(id);
  if (!el) return;
  el.style.transform = dx || dy ? `translate3d(${dx}px, ${dy}px, 0)` : '';
  touched.add(id);
}

/**
 * A block drawn with a different left edge and width while one of its edges is dragged. `--trim-dx` says how far its
 * left edge moved, so its filmstrip and waveform move back by as much and stay put in time (`media/BlockMedia`).
 */
export function sizeBlock(id: string, leftPx: number, widthPx: number) {
  const el = blocks.get(id);
  if (!el) return;
  el.style.left = `${leftPx}px`;
  el.style.width = `${Math.max(1, widthPx)}px`;
  el.style.setProperty('--trim-dx', `${leftPx - Number(el.dataset.left ?? leftPx)}px`);
  el.dataset.trimming = 'true';
  touched.add(id);
}

export function clearShifts() {
  for (const id of touched) {
    const el = blocks.get(id);
    if (!el) continue;
    el.style.transform = '';
    if (el.dataset.trimming) {
      // React's last render is in `data-left`/`data-width`: React won't rewrite a style it thinks is unchanged.
      el.style.left = `${el.dataset.left}px`;
      el.style.width = `${el.dataset.width}px`;
      el.style.removeProperty('--trim-dx');
      delete el.dataset.trimming;
    }
  }
  touched.clear();
}
