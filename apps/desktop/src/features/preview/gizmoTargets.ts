import type { BoxN } from '../../bindings/BoxN';
import type { FormatKind } from '../../bindings/FormatKind';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Layout } from '../../bindings/Layout';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import { containRect, FORMAT_SIZE, pictureRect, resolveRect } from '../../lib/layout';
import { showsPictures, soloOf } from '../../lib/solo';
import { scaledStyle } from '../../lib/textLook';
import { assetById, itemEndUs, itemLabel } from '../../lib/timeline';
import { contentNow, layoutNow } from '../../state/latest';
import { textBox } from './textBox';

export interface GizmoTarget {
  /** Selection id. */
  id: string;
  /** A title template (a motion graphic that fills the frame) is a `scene`. */
  label: 'text' | 'image' | 'video' | 'scene';
  /** Spoken and shown name, e.g. the title's words. */
  name: string;
  layout: Layout;
  /** Its layout box in the preview format (what a move changes). */
  rect: BoxN;
  /** What the engine draws, before rotation: `rect`, or the picture's shape inside it for images and videos. */
  box: BoxN;
  /**
   * What the engine draws for a layout box (`box` is this for `rect`), with text at `fontSize` if given: resizes
   * and nudges redraw with it.
   */
  boxFor: (rect: BoxN, fontSize?: number) => BoxN;
  locked: boolean;
  /** False for a title template: it fills the frame, and its parts are placed inside it. */
  movable: boolean;
  rotatable: boolean;
  /**
   * A text layer's font size, which its corners scale in the primary format. Null for everything else, and for text
   * in another format: there the font follows the box's height (`textScale`), so only the box changes.
   */
  fontSize: number | null;
  /**
   * The ops that place this target in one change: `place` turns its layout, as the project has it when the ops are
   * built, into the placed one (its box and turn only), and for text `fontSize` sets its size and outline on its style
   * then. Built at the change's turn, a change to the layer still on its way (another control's, an opacity, a colour)
   * is kept, never put back by the layout or content the gesture's render saw.
   */
  ops: (place: (layout: Layout) => Layout, fontSize?: number) => Op[];
}

const same = (rect: BoxN) => rect;

/**
 * Where a title template's own picture is in the frame, for picking it: the lower third's card (a share of the frame
 * across, 238 of 1080p's pixels tall, as its `scene.json` places it, inside 9:16's clear area), the title card's whole
 * frame. Anything else the layout's box.
 */
const SCENE_PICK: Record<string, (format: FormatKind) => BoxN> = {
  'klipolo.builtin/scene.lower-third': (format) => ({
    x: 0.06,
    y: 0.58,
    w: 0.56,
    h: 238 / FORMAT_SIZE[format][1],
  }),
};

/**
 * Everything that can be picked on the canvas at `playheadUs`, bottom to top (overlays in their tracks' order). It
 * lists exactly what the frame plan draws, at the same rects (pinned by the layout parity test).
 */
export function gizmoTargets(project: Project, playheadUs: number, format: FormatKind): GizmoTarget[] {
  const primary = project.settings.primaryFormat;
  const targets: GizmoTarget[] = [];
  // A track solo leaves out isn't drawn, so nothing on it can be picked.
  const solo = soloOf(project.timeline.tracks);
  for (const track of project.timeline.tracks) {
    if (!showsPictures(solo, track)) continue;
    for (const item of track.items) {
      if (playheadUs < item.startUs || playheadUs >= itemEndUs(item)) continue;
      const content = item.content;
      const scene = content.type === 'generator' ? content : null;
      if (content.type !== 'text' && content.type !== 'image' && content.type !== 'video' && !scene) continue;
      const rect = resolveRect(item.layout, primary, format);
      let boxFor: GizmoTarget['boxFor'] = same;
      const text = content.type === 'text' ? content : null;
      if (text) {
        boxFor = textBoxes(item.layout, text, primary, format);
      } else if (content.type === 'image' || content.type === 'video') {
        // Missing media draws nothing, so there is nothing to pick.
        const asset = assetById(project, content.assetId);
        if (asset?.kind !== content.type) continue;
        boxFor = (r) =>
          (content.type === 'image' ? pictureRect : containRect)(r, asset.width, asset.height, format);
      } else if (scene) {
        const pick = SCENE_PICK[scene.source];
        if (pick) boxFor = () => pick(format);
      }
      targets.push({
        id: item.id,
        label: scene ? 'scene' : (content.type as 'text' | 'image' | 'video'),
        name: itemLabel(project, item),
        layout: item.layout,
        rect,
        box: boxFor(rect),
        boxFor,
        locked: track.locked,
        movable: !scene,
        rotatable: !scene,
        fontSize: text && format === primary ? text.style.size : null,
        ops: (place, size) => {
          const layout = place(layoutNow(item.id, item.layout));
          // Only a gesture that changed the size (from the one it began on) sets it: by as much, on the size the
          // title has then, so a size still settling as the drag lands keeps its step too.
          if (!text || size === undefined || size === text.style.size)
            return [{ type: 'updateItem', id: item.id, patch: { layout } }];
          const now = contentNow(item.id, text);
          // Within the model's range, on half pixels, as `fontSizeFor`.
          const grown =
            now.style.size === text.style.size
              ? size
              : Math.min(400, Math.max(8, Math.round(((now.style.size * size) / text.style.size) * 2) / 2));
          const style = scaledStyle(now.style, grown);
          return [{ type: 'updateItem', id: item.id, patch: { layout, content: { ...now, style } } }];
        },
      });
    }
  }
  return targets;
}

type TextContent = Extract<ItemContent, { type: 'text' }>;

/**
 * What a text layer draws for a layout box in `format` (taller than the box where its lines overflow it). Its font
 * is `size` (default: its own) in the primary format; elsewhere it follows the box's height (`textScale`).
 */
function textBoxes(layout: Layout, text: TextContent, primary: FormatKind, format: FormatKind) {
  const [W, H] = FORMAT_SIZE[format];
  const [pw, ph] = FORMAT_SIZE[primary];
  const short = Math.min(pw, ph) / 1080;
  const base = resolveRect(layout, primary, primary).h * ph;
  return (r: BoxN, size?: number) => {
    const fontPx =
      format === primary || !(base > 0)
        ? (size ?? text.style.size) * short
        : ((text.style.size * r.h * H) / base) * short;
    return textBox(r, text.text, text.style, fontPx, W, H);
  };
}
