/**
 * The block elements on screen, by id. Gestures move and size them directly (transforms, one element's width), so a
 * drag never re-renders React; `clearShifts` puts every touched block back once the new revision has drawn.
 */
const blocks = new Map<string, HTMLElement>();
/** What is drawn beside a block and goes with it (music's ducking tag), by the block's id. */
const followers = new Map<string, HTMLElement>();
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

/**
 * Records `el` as drawn beside block `id` (a ref callback, as `registerBlock`): a move of the block moves it too, and
 * while the block's edges are dragged it hides, since it was placed for the block's old length.
 */
export function registerFollower(id: string, el: HTMLElement | null): () => void {
  if (el) followers.set(id, el);
  return () => {
    if (el && followers.get(id) === el) followers.delete(id);
  };
}

/** Where a box is on screen (client px). */
export interface ScreenBox {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * The clips and layers whose blocks a box drawn over the lanes touches (an edge on its edge counts), in the order they
 * were drawn. Cuts, transitions and roll handles (ids with a ':') aren't blocks to select. O(mounted blocks).
 */
export function blocksTouching(box: ScreenBox): string[] {
  const out: string[] = [];
  for (const [id, el] of blocks) {
    if (id.includes(':')) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    if (box.left <= r.right && box.right >= r.left && box.top <= r.bottom && box.bottom >= r.top) out.push(id);
  }
  return out;
}

export function blockElement(id: string): HTMLElement | null {
  return blocks.get(id) ?? null;
}

export function shiftBlock(id: string, dx: number, dy = 0) {
  const el = blocks.get(id);
  if (!el) return;
  el.style.transform = dx || dy ? `translate3d(${dx}px, ${dy}px, 0)` : '';
  const follower = followers.get(id);
  if (follower) follower.style.transform = el.style.transform;
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
  const follower = followers.get(id);
  if (follower) follower.style.visibility = 'hidden';
  touched.add(id);
}

/**
 * A clip whose footage slips: drawn where it is, its filmstrip and waveform moved `px` the other way (`--trim-dx`), as
 * the footage now in it plays there.
 */
export function slipBlock(id: string, px: number) {
  const el = blocks.get(id);
  if (!el) return;
  el.style.setProperty('--trim-dx', `${px}px`);
  el.dataset.trimming = 'true';
  touched.add(id);
}

export function clearShifts() {
  for (const id of touched) {
    const follower = followers.get(id);
    if (follower) {
      follower.style.transform = '';
      follower.style.visibility = '';
    }
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
