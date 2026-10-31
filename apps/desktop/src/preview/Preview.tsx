import { useEffect, useRef, type RefObject } from 'react';
import type { EngineInfo } from '../bindings/EngineInfo';
import type { FormatKind } from '../bindings/FormatKind';
import { engine } from '../bridge/engine';
import { AnalysisBanner } from '../features/analysis/AnalysisBanner';
import { RoughCaptionsNote } from '../features/analysis/RoughCaptionsNote';
import { FormatPills } from '../features/editor/FormatPills';
import { useShownFormat } from '../features/editor/formats';
import { useImportMedia } from '../features/media/useImportMedia';
import { Icon } from '../layout/Icon';
import { useEngine } from '../state/engine';
import { useProject } from '../state/project';
import { SocketCanvas } from './SocketCanvas';
import { ASPECT, holeRect, previewPixels } from './viewSize';

/** `--color-night` as 0xRRGGBB: the window shows it around the native layer for a frame or two while it resizes. */
const NIGHT = 0x070a14;

/**
 * Tells the engine what to render: the format, at the frame box's size in device pixels (within the engine's
 * budget). A drag of the window settles before the size is sent again.
 */
function useViewSync(
  frame: RefObject<HTMLDivElement | null>,
  info: EngineInfo | null,
  format: FormatKind,
  mounted: boolean,
) {
  useEffect(() => {
    const el = frame.current;
    if (!el || !mounted || !info || info.port === 0) return;
    let sent = '';
    let timer: ReturnType<typeof setTimeout> | undefined;
    const send = () => {
      const r = el.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) return;
      const px = previewPixels(r, window.devicePixelRatio || 1, info.maxPreviewPixels);
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
  }, [frame, info, format, mounted]);
}

/**
 * The native presenter's hole: the frame box is transparent, the engine's layer sits under the webview exactly there,
 * and everything else still draws above it in HTML. Its rect goes to Rust at most once per animation frame, on layout,
 * resize and display changes.
 */
function useNativeHole(frame: RefObject<HTMLDivElement | null>, on: boolean) {
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
      void engine.setRect(rect && { ...rect, backdrop: NIGHT }).catch(() => undefined);
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
    request();
    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
      window.removeEventListener('resize', resized);
      void engine.setRect(null).catch(() => undefined);
    };
  }, [frame, on]);
}

/** The format the preview shows (it lives with the format switch). */
export { useShownFormat };

/**
 * An empty project's drop zone: drop videos anywhere on the window, or choose them (the one red), or
 * files are held over the window.
 */
function DropZone({ over }: { over: boolean }) {
  const importMedia = useImportMedia();
  return (
    <div className={over ? 'drop is-over' : 'drop'}>
      <div className="drop-art" aria-hidden>
        <b>
          <Icon name="plus" />
        </b>
      </div>
      <h1 className="drop-title">Drop videos here</h1>
      <p className="drop-text">MOV, MP4 or M4V. Add several at once and they line up in the order you drop them.</p>
      <div className="drop-actions">
        <button className="btn btn-primary btn-lg" onClick={() => void importMedia()}>
          Choose files
        </button>
        <span>
          or press <kbd>&#8984;</kbd> <kbd>O</kbd>
        </span>
      </div>
    </div>
  );
}

/**
 * The preview panel: the format pills and the engine's frames, fitted to the panel in the chosen format, under the
 * analysis banner; with no clip yet, the drop zone. `filesOver`: files from Finder are held over the window.
 */
export function Preview({ filesOver = false }: { filesOver?: boolean }) {
  const info = useEngine((s) => s.info);
  const native = useEngine((s) => s.presenter === 'native' && !!s.info);
  const status = useEngine((s) => s.status);
  const message = useEngine((s) => s.message);
  const hasVideo = useProject((s) => (s.snapshot?.project.timeline.main.length ?? 0) > 0);
  const format = useShownFormat();
  const frame = useRef<HTMLDivElement>(null);
  useViewSync(frame, info, format, hasVideo);
  useNativeHole(frame, native && hasVideo);

  const note = status === 'unavailable' ? (message ?? "The preview isn't available right now.") : null;
  return (
    <main className={native && hasVideo ? 'panel stage has-hole' : 'panel stage'} aria-label="Preview">
      <div className="stage-head">
        <FormatPills />
        <span className="spacer" />
        {status !== 'unavailable' && message && <span className="stage-status">{message}</span>}
        <RoughCaptionsNote />
        <button className="icon-btn" aria-label="Show safe area" title="Safe area" disabled>
          <Icon name="safe" />
        </button>
        <button className="icon-btn" aria-label="Full screen preview" title="Full screen (F)" disabled>
          <Icon name="full" />
        </button>
      </div>
      <div className="stage-body">
        {hasVideo && <AnalysisBanner />}
        {hasVideo ? (
          <div className="screen">
            <div
              ref={frame}
              className={`frame frame-${format}${native ? '' : ' is-socket'}`}
              data-testid="preview-frame"
            >
              {info && !native && <SocketCanvas info={info} aspect={ASPECT[format]} />}
              {note && <div className="preview-note">{note}</div>}
            </div>
          </div>
        ) : (
          <DropZone over={filesOver} />
        )}
      </div>
    </main>
  );
}
