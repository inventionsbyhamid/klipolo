import type { EngineInfo } from './engine';
import { readTrailer, type FrameMeta } from '../preview/frameTrailer';

export interface FrameSocket {
  close(): void;
}

/** Connects to the engine's frame server, retrying up to 3 times (250, 500, 1000 ms) before reporting failure. */
export function openFrameSocket(
  info: EngineInfo,
  onFrame: (buf: ArrayBuffer, meta: FrameMeta) => void,
  onFailed: () => void,
): FrameSocket {
  let ws: WebSocket | null = null;
  let tries = 0;
  let closed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const connect = () => {
    ws = new WebSocket(`ws://127.0.0.1:${info.port}/frames?t=${info.token}`);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => {
      tries = 0;
    };
    ws.onmessage = (e: MessageEvent) => {
      if (!(e.data instanceof ArrayBuffer)) return;
      const meta = readTrailer(e.data);
      if (meta) onFrame(e.data, meta);
    };
    ws.onclose = () => {
      if (closed) return;
      tries += 1;
      if (tries > 3) return onFailed();
      timer = setTimeout(connect, 125 * 2 ** tries);
    };
  };
  connect();
  return {
    close() {
      closed = true;
      clearTimeout(timer);
      ws?.close();
    },
  };
}
