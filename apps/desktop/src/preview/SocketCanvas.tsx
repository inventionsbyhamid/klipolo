import { useEffect, useRef } from 'react';
import type { EngineInfo } from '../bridge/engine';
import { openFrameSocket } from '../bridge/frameSocket';
import { frameDrawn, getSession, socketFailed, useSession } from '../player/session';
import { FrameQueue, REFRESH_MS } from './framePick';
import { createRenderer } from './frameRenderer';
import type { FrameMeta } from './frameTrailer';

/** A frame of another shape belongs to the format shown before a switch: drawn stretched, it would look squashed. */
function sameShape(meta: FrameMeta, aspect: readonly [number, number]): boolean {
  return Math.abs((meta.width * aspect[1]) / (meta.height * aspect[0]) - 1) < 0.03;
}

/**
 * The socket presenter's canvas: frames arrive over the websocket and wait in a short queue; each animation frame
 * draws the newest one due by the next refresh, so arrival jitter never becomes judder. The playhead follows the time
 * of each drawn frame.
 */
export function SocketCanvas({ info, aspect }: { info: EngineInfo; aspect: readonly [number, number] }) {
  const ref = useRef<HTMLDivElement>(null);
  const shape = useRef(aspect);
  shape.current = aspect;
  const live = useSession((s) => s.status === 'live');
  useEffect(() => {
    const host = ref.current;
    if (!host || info.port === 0) return;
    const renderer = createRenderer(host);
    const queue = new FrameQueue();
    let raf = 0;
    const show = (buf: ArrayBuffer, meta: FrameMeta) => {
      if (!sameShape(meta, shape.current)) return;
      renderer.draw(buf, meta);
      frameDrawn(meta.timeUs, meta.seq);
    };
    const tick = (at: number) => {
      raf = 0;
      // What this animation frame draws reaches the screen on the next refresh (due times are wall clock).
      const frame = queue.take(performance.timeOrigin + at + REFRESH_MS);
      if (frame) show(frame.buf, frame.meta);
      if (queue.size) raf = requestAnimationFrame(tick);
    };
    const enqueue = (buf: ArrayBuffer, meta: FrameMeta) => {
      queue.push({ buf, meta });
      if (!raf) raf = requestAnimationFrame(tick);
    };
    // A frame can arrive before its session's `ready` event: keep the newest such frame until the session matches.
    let early: [ArrayBuffer, FrameMeta] | null = null;
    const waitForSession = setInterval(() => {
      if (!early || early[1].session !== getSession().session) return;
      const [buf, meta] = early;
      early = null;
      enqueue(buf, meta);
    }, 50);
    const socket = openFrameSocket(
      info,
      (buf, meta) => {
        if (meta.session !== getSession().session) {
          early = [buf, meta];
          return;
        }
        early = null;
        enqueue(buf, meta);
      },
      socketFailed,
    );
    return () => {
      cancelAnimationFrame(raf);
      clearInterval(waitForSession);
      socket.close();
      renderer.dispose();
    };
  }, [info]);
  return <div ref={ref} aria-hidden className={live ? 'frame-canvas-host' : 'frame-canvas-host is-hidden'} />;
}
