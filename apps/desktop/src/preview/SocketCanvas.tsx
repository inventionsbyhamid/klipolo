import { useEffect, useRef } from 'react';
import type { EngineInfo } from '../bindings/EngineInfo';
import { engine } from '../bridge/engine';
import { openFrameSocket } from '../bridge/frameSocket';
import { epochAt } from '../lib/clocks';
import { isCurrentFrame, useEngine } from '../state/engine';
import { usePlayback } from '../state/playback';
import { FrameQueue, REFRESH_MS } from './framePick';
import { createRenderer } from './frameRenderer';
import { FLAG_PLAYBACK, type FrameMeta } from './frameTrailer';
import { presentDelay } from './presentDelay';

/** A frame of another shape belongs to the format shown before a switch: drawn stretched, it would look squashed. */
function sameShape(meta: FrameMeta, aspect: readonly [number, number]): boolean {
  return Math.abs((meta.width * aspect[1]) / (meta.height * aspect[0]) - 1) < 0.03;
}

/**
 * The socket presenter's canvas: frames arrive over the websocket and wait in a short queue; each animation frame
 * draws the newest one due by the next refresh, so arrival jitter never becomes judder. The playback clock follows the
 * time of each drawn frame. While playing, how long frames take to arrive goes to the engine about once a second
 * (`presentDelay`), so it renders them that far ahead of the sound.
 */
export function SocketCanvas({ info, aspect }: { info: EngineInfo; aspect: readonly [number, number] }) {
  const ref = useRef<HTMLDivElement>(null);
  const shape = useRef(aspect);
  shape.current = aspect;
  const live = useEngine((s) => s.status === 'live');
  useEffect(() => {
    const host = ref.current;
    if (!host || info.port === 0) return;
    const renderer = createRenderer(host);
    const queue = new FrameQueue();
    const delivery = presentDelay();
    let raf = 0;
    const show = (buf: ArrayBuffer, meta: FrameMeta) => {
      if (!sameShape(meta, shape.current)) return;
      renderer.draw(buf, meta);
      usePlayback.getState().onFrame({
        timeUs: meta.timeUs,
        seq: meta.seq,
        playback: (meta.flags & FLAG_PLAYBACK) !== 0,
      });
      const engine = useEngine.getState();
      if (engine.status !== 'live') engine.setStatus('live');
    };
    const tick = (at: number) => {
      raf = 0;
      // What this animation frame draws reaches the screen on the next refresh (due times are wall clock; `epochAt`
      // stays right after the Mac slept, when the performance clock lags it).
      const frame = queue.take(epochAt(at) + REFRESH_MS);
      if (frame) show(frame.buf, frame.meta);
      if (queue.size) raf = requestAnimationFrame(tick);
    };
    const enqueue = (buf: ArrayBuffer, meta: FrameMeta) => {
      queue.push({ buf, meta });
      if (!raf) raf = requestAnimationFrame(tick);
    };
    // A frame can arrive before its session's `ready` event: keep the newest such frame until the session matches.
    let early: [ArrayBuffer, FrameMeta] | null = null;
    const unsubscribe = useEngine.subscribe((s, prev) => {
      if (s.session === prev.session || !early || early[1].session !== s.session) return;
      const [buf, meta] = early;
      early = null;
      enqueue(buf, meta);
    });
    const socket = openFrameSocket(
      info,
      (buf, meta) => {
        if (!isCurrentFrame(meta)) {
          early = [buf, meta];
          return;
        }
        early = null;
        const delay = delivery(meta);
        if (delay !== null) void engine.presentDelay(delay).catch(() => undefined);
        enqueue(buf, meta);
      },
      () => useEngine.getState().setStatus('unavailable', "The preview isn't available right now."),
    );
    return () => {
      cancelAnimationFrame(raf);
      unsubscribe();
      socket.close();
      renderer.dispose();
    };
  }, [info]);
  return <div ref={ref} aria-hidden className={live ? 'frame-canvas-host' : 'frame-canvas-host is-hidden'} />;
}
