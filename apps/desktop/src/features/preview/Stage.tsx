import { useEffect, useMemo, useRef, type RefObject } from 'react';
import type { EngineInfo } from '../../bindings/EngineInfo';
import type { FormatKind } from '../../bindings/FormatKind';
import { engine } from '../../bridge/engine';
import { FORMAT_SIZE } from '../../lib/layout';
import type { Size } from '../../lib/preview';
import { ASPECT, holeRect, previewPixels } from '../../preview/viewSize';
import { SocketCanvas } from '../../preview/SocketCanvas';
import { useEngine } from '../../state/engine';
import { useFullScreen } from '../../state/fullScreen';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { useShownFormat } from '../editor/formats';
import { clipIndexAt, mainIndex } from '../timeline/model/index';
import { GizmoLayer } from './GizmoLayer';
import { sceneStartUs } from './sceneStart';
import { cardAnchor, selectedBox, stageLayout } from './stageLayout';
import { useDevicePixelRatio } from './useDevicePixelRatio';
import { useSettledSize } from './useSettledSize';

/** Whether `fitBox` of a `width` x `height` container is `box`, with no allocation (it runs per resize callback). */
function fitsAs(width: number, height: number, [w, h]: readonly [number, number], box: Size): boolean {
  const ratio = w / h;
  const fit = Math.max(0, Math.min(width, height * ratio));
  return Math.round(fit) === box.width && Math.round(fit / ratio) === box.height;
}

export { useShownFormat };

/** The card's width and the gap before its slot (CSS px), from the design tokens. */
function cardSize(el: Element | null): { width: number; gap: number } {
  const style = el ? getComputedStyle(el) : null;
  const px = (name: string, fallback: number) => parseFloat(style?.getPropertyValue(name) ?? '') || fallback;
  return { width: px('--inspector-w', 264), gap: px('--space-6', 16) };
}

/**
 * Tells the engine what to render: the format, at the frame box's size in device pixels (within the engine's
 * budget). A drag of the window settles before the size is sent again. A move to a screen of another pixel ratio
 * (`dpr`) sends it at once, resized or not.
 */
function useViewSync(
  frame: RefObject<HTMLDivElement | null>,
  info: EngineInfo | null,
  format: FormatKind,
  dpr: number,
) {
  useEffect(() => {
    const el = frame.current;
    if (!el || !info || info.port === 0) return;
    let sent = '';
    let timer: ReturnType<typeof setTimeout> | undefined;
    const send = () => {
      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) return;
      const px = previewPixels(r, dpr, info.maxPreviewPixels);
      const key = `${format} ${px.width}x${px.height}`;
      if (key === sent) return;
      sent = key;
      void engine.setView({ format, ...px }).catch(() => undefined);
    };
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(send, sent ? 150 : 0);
    });
    observer.observe(el);
    send();
    return () => {
      clearTimeout(timer);
      observer.disconnect();
    };
  }, [frame, info, format, dpr]);
}

/**
 * The native presenter's hole: the frame box is transparent, the engine's layer sits under the webview exactly there,
 * and everything else still draws above it in HTML. Its rect goes to Rust at most once per animation frame, on layout,
 * resize and display changes, and again on a move to a screen of another pixel ratio (its scale). It leaves the
 * window's own colour (what shows around the layer for a frame or two while the window resizes) to the theme: Rust
 * paints it with the theme's canvas (`setAppearance`).
 */
function useNativeHole(frame: RefObject<HTMLDivElement | null>, on: boolean, dpr: number) {
  const rescale = useRef<(() => void) | null>(null);
  useEffect(() => {
    const el = frame.current;
    if (!el || !on) return;
    let raf = 0;
    let forced = false;
    let last = '';
    const measure = () => {
      raf = 0;
      const rect = holeRect(el, window.devicePixelRatio || 1);
      const key = JSON.stringify(rect);
      if (!forced && key === last) return;
      forced = false;
      last = key;
      void engine.setRect(rect && { ...rect, backdrop: null }).catch(() => undefined);
    };
    const request = (force = false) => {
      forced ||= force;
      if (!raf) raf = requestAnimationFrame(measure);
    };
    // Any ancestor's size change can move the box (panels, the window).
    const observer = new ResizeObserver(() => request());
    for (let node: Element | null = el; node; node = node.parentElement) observer.observe(node);
    // A window resize may end with the hole where it was while AppKit moved the view: send it again.
    const resized = () => request(true);
    window.addEventListener('resize', resized);
    rescale.current = () => request(true);
    request();
    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
      window.removeEventListener('resize', resized);
      rescale.current = null;
      void engine.setRect(null).catch(() => undefined);
    };
  }, [frame, on]);
  // Another screen's pixel ratio: the same box sent again at the new scale, never hidden on the way.
  useEffect(() => {
    rescale.current?.();
  }, [dpr]);
}

type Pressed = 'video' | 'stage' | null;

/**
 * What a press began on: the bare video, the stage around it, or something with its own click (a layer, the failure
 * note's Try again). Judged at press time, so a drag that ends off its layer never reads as a click on what lies under
 * the pointer.
 */
function pressedOn(target: EventTarget, stage: Element, video: Element | null): Pressed {
  if (target === stage) return 'stage';
  if (!(target instanceof Element) || !video?.contains(target) || target.closest('[data-layer], button')) return null;
  return 'video';
}

/**
 * The project at the playhead in the preview format: the engine's frames, fitted to the panel, with the
 * layers there selectable on top (outline, handles, the text typed in place). Clicking the bare video selects the clip
 * at the playhead; clicking around it clears the selection. It re-renders when a layer enters or leaves at the
 * playhead (`sceneStartUs`), never per playback frame.
 *
 * While its area resizes the video box follows in CSS, with no render, and the handles hide while the box's size
 * differs from the one drawn; it re-renders once, at the settled size (`useSettledSize`).
 *
 * Only the area and the format place the video, never the selection (`stageLayout`): the card that a selection shows
 * goes in a slot kept clear at the area's right (`data-card="slot"`), or over the video (`"over"`), in the stage's corner
 * that covers the selected layer least (`data-anchor`), so selecting never resizes the video or the engine's drawable.
 */
export function Stage() {
  const project = useProject((s) => s.snapshot!.project);
  const sceneUs = usePlayback((s) => sceneStartUs(project, s.playheadUs));
  const select = useSelection((s) => s.select);
  const format = useShownFormat();
  const info = useEngine((s) => s.info);
  const native = useEngine((s) => s.presenter === 'native' && !!s.info);
  const status = useEngine((s) => s.status);
  const message = useEngine((s) => s.message);
  const areaRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLDivElement>(null);
  const handlesRef = useRef<HTMLDivElement>(null);
  const pressed = useRef<Pressed>(null);
  // The handles hide only while the box the area fits differs from the one drawn: a box that only moves (9:16 in a
  // wider area) keeps them, and they follow it in CSS.
  const area = useSettledSize(areaRef, (on, width, height) =>
    handlesRef.current?.toggleAttribute(
      'data-settling',
      on && !fitsAs(width - pad, height, FORMAT_SIZE[format], canvas),
    ),
  );
  // The tokens change with the window's width, which changes the area too. Full screen shows no card: no slot.
  const full = useFullScreen((s) => s.on);
  const size = useMemo(() => cardSize(areaRef.current), [area]);
  const { card, pad, canvas } = stageLayout(area, FORMAT_SIZE[format], full ? 0 : size.width + size.gap);
  // Over the video, the card's corner follows the selected layer as the project has it: never mid-drag.
  const anchor = useSelection((s) =>
    card === 'over' ? cardAnchor(selectedBox(project, s.selectedId, format), area, canvas, size.width) : undefined,
  );
  const dpr = useDevicePixelRatio();
  useViewSync(videoRef, info, format, dpr);
  useNativeHole(videoRef, native, dpr);
  const note = status === 'unavailable' ? (message ?? "The preview isn't available right now.") : null;
  return (
    <div
      ref={areaRef}
      data-testid="stage-area"
      className="screen"
      data-card={card}
      data-anchor={anchor}
      style={pad > 0 ? { paddingRight: pad } : undefined}
      onPointerDown={(e) => {
        pressed.current = pressedOn(e.target, e.currentTarget, videoRef.current);
      }}
      onClick={() => {
        const on = pressed.current;
        pressed.current = null;
        const at = usePlayback.getState().playheadUs;
        const { main } = project.timeline;
        const clip = clipIndexAt(mainIndex(main), at);
        if (on === 'video') select(clip >= 0 ? main[clip].id : null);
        else if (on === 'stage') select(null);
      }}
    >
      <div
        ref={videoRef}
        data-testid="stage"
        className={`frame frame-${format}${native ? '' : ' is-socket'}`}
      >
        {info && !native && <SocketCanvas info={info} aspect={ASPECT[format]} />}
        {note && (
          // The way back without closing the project: the engine is attached again.
          <div className="preview-note">
            <p>{note}</p>
            <button
              type="button"
              // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
              tabIndex={0}
              className="btn btn-secondary btn-sm"
              onClick={() => useEngine.getState().retry()}
            >
              Try again
            </button>
          </div>
        )}
      </div>
      {/* The same box, unclipped, so a selection at the canvas's edge keeps its handles. Hidden (never `visibility:
          hidden`, which would take focus off a layer or the title editor) and out of reach while the size settles: the
          handles are placed for the size before. */}
      <div ref={handlesRef} data-testid="stage-handles" className={`frame-handles frame-${format}`}>
        <GizmoLayer project={project} format={format} playheadUs={sceneUs} canvas={canvas} />
      </div>
    </div>
  );
}
