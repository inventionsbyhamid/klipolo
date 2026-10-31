import type { BoxN } from '../../bindings/BoxN';
import type { TextStyle } from '../../bindings/TextStyle';
import { ensureFace, faceCss, onFaceAdded } from '../../lib/fonts';

/** The engine's line spacing in font sizes (`LINE_HEIGHT` in crates/klipolo-engine/src/render/text.rs). */
const LINE_HEIGHT = 1.2;
/** Heights kept for reuse: gizmo boxes are worked out again on every playhead move while playing. */
const MAX_KEPT = 128;

const heights = new Map<string, number>();
let probe: HTMLDivElement | null = null;

// A bundled face joining the page lays its family's text out differently: measure again.
onFaceAdded(() => heights.clear());

function makeProbe(): HTMLDivElement {
  const el = document.createElement('div');
  el.setAttribute('aria-hidden', 'true');
  Object.assign(el.style, {
    position: 'absolute',
    left: '-100000px',
    top: '0',
    visibility: 'hidden',
    pointerEvents: 'none',
    whiteSpace: 'pre-wrap',
    // The engine breaks a word wider than the wrap width between its letters (`Wrap::WordOrGlyph`).
    overflowWrap: 'break-word',
    lineHeight: String(LINE_HEIGHT),
    contain: 'layout style',
    fontOpticalSizing: 'none',
  });
  document.body.append(el);
  return el;
}

/**
 * The width the engine wraps a text box `boxW` wide in: the box plus 5 % and 4 px of slack, always. (`shaping` judges
 * its 98 % rule against the text's own texture, the box plus its padding, which the box never fills: so a full-width
 * title in 9:16 wraps there too, never at the frame's width.) The 4 px are the engine's own pixels: `unit` is how many
 * of them a pixel of `boxW` is (the preview's device pixels per CSS pixel: `previewScale`; 1 for the export's).
 */
export function wrapWidth(boxW: number, unit = 1): number {
  return boxW * 1.05 + 4 / unit;
}

/**
 * How tall the engine lays `text` out, in the pixels `fontPx` and `boxW` are in: wrapped by word at the engine's
 * wrap width (`wrapWidth`), a word wider than that broken between its letters as the engine breaks it, 1.2 font sizes
 * per line, in the text's bundled face (Inter until it has loaded, then measured again). Measured in one hidden
 * element and cached; 0 where nothing lays out (tests).
 */
export function textHeight(text: string, style: TextStyle, fontPx: number, boxW: number): number {
  if (!text || typeof document === 'undefined') return 0;
  void ensureFace(style);
  const wrap = wrapWidth(boxW);
  const id = `${style.font.family}|${style.weight}|${style.italic ? 'i' : 'n'}|${fontPx.toFixed(1)}|${wrap.toFixed(0)}|${text}`;
  const kept = heights.get(id);
  if (kept !== undefined) return kept;
  probe ??= makeProbe();
  const face = faceCss(style);
  probe.style.width = `${wrap}px`;
  probe.style.fontSize = `${fontPx}px`;
  probe.style.fontFamily = String(face.fontFamily);
  probe.style.fontWeight = String(face.fontWeight);
  probe.style.fontStyle = String(face.fontStyle);
  probe.textContent = text;
  const height = probe.offsetHeight;
  probe.textContent = ''; // the words live on the canvas, not twice in the page
  heights.set(id, height);
  if (heights.size > MAX_KEPT) heights.delete(heights.keys().next().value!);
  return height;
}

/**
 * What a text layer at `rect` draws on a `width`x`height` canvas: its box, or taller where its lines overflow it (the
 * engine centres them on the box, so the overflow is shared above and below).
 */
export function textBox(
  rect: BoxN,
  text: string,
  style: TextStyle,
  fontPx: number,
  width: number,
  height: number,
): BoxN {
  const h = textHeight(text, style, fontPx, rect.w * width) / height;
  return h <= rect.h ? rect : { ...rect, y: rect.y + (rect.h - h) / 2, h };
}
