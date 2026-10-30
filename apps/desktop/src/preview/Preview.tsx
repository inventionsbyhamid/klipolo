import { useEffect, useRef, type RefObject } from 'react';
import { engineSetView, engineView, type EngineInfo, type FormatKind } from '../bridge/engine';
import { Icon } from '../layout/Icon';
import { setFormat, useSession } from '../player/session';
import { SocketCanvas } from './SocketCanvas';
import { ASPECT, holeRect, previewPixels } from './viewSize';

const FORMATS: [FormatKind, string][] = [
  ['landscape', '16:9'],
  ['portrait', '9:16'],
  ['square', '1:1'],
];

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
      void engineSetView({ format, ...px }).catch(() => undefined);
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
      void engineView(rect && { ...rect, backdrop: NIGHT }).catch(() => undefined);
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
      void engineView(null).catch(() => undefined);
    };
  }, [frame, on]);
}

/** The preview panel: the format pills and the engine's frames, fitted to the panel in the chosen format. */
export function Preview() {
  const info = useSession((s) => s.info);
  const native = useSession((s) => s.presenter === 'native' && !!s.info);
  const format = useSession((s) => s.format);
  const hasProject = useSession((s) => !!s.project);
  const status = useSession((s) => s.status);
  const message = useSession((s) => s.message);
  const frame = useRef<HTMLDivElement>(null);
  useViewSync(frame, info, format, hasProject);
  useNativeHole(frame, native && hasProject);

  const note = status === 'unavailable' ? (message ?? "The preview isn't available right now.") : null;
  return (
    <main className={native && hasProject ? 'panel stage has-hole' : 'panel stage'} aria-label="Preview">
      <div className="stage-head">
        <div className="seg" role="radiogroup" aria-label="Format">
          {FORMATS.map(([kind, label]) => (
            <button key={kind} role="radio" aria-checked={format === kind} onClick={() => setFormat(kind)}>
              {label}
            </button>
          ))}
        </div>
        <span className="spacer" />
        {status !== 'unavailable' && message && <span className="stage-status">{message}</span>}
        <button className="icon-btn" aria-label="Show safe area" title="Safe area" disabled>
          <Icon name="safe" />
        </button>
        <button className="icon-btn" aria-label="Full screen preview" title="Full screen (F)" disabled>
          <Icon name="full" />
        </button>
      </div>
      <div className="stage-body">
        <div className="screen">
          {hasProject ? (
            <div
              ref={frame}
              className={`frame frame-${format}${native ? '' : ' is-socket'}`}
              data-testid="preview-frame"
            >
              {info && !native && <SocketCanvas info={info} aspect={ASPECT[format]} />}
              {note && <div className="preview-note">{note}</div>}
            </div>
          ) : (
            <div className="screen-empty">
              <h1>No video yet</h1>
              <p>{note ?? 'Open Klipolo with a video to preview it here. Importing comes next.'}</p>
            </div>
          )}
        </div>
      </div>
    </main>
  );
}
