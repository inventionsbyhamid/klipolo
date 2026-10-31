import type { PreviewFrame } from '../../bindings/PreviewFrame';
import type { PreviewKind } from '../../bindings/PreviewKind';

/** Headers are UTF-8 JSON; one decoder serves every frame. */
const text = new TextDecoder();

const KINDS: ReadonlySet<string> = new Set<PreviewKind>(['thumb', 'live', 'camera']);

/** A preview frame as the page reads it: its header, and its JPEG as a `Blob` (drawn from an object URL). */
export interface PreviewMessage {
  frame: PreviewFrame;
  jpeg: Blob;
}

function isFrame(value: unknown): value is PreviewFrame {
  if (typeof value !== 'object' || value === null) return false;
  const { kind, key, atMs } = value as Record<string, unknown>;
  return typeof kind === 'string' && KINDS.has(kind) && typeof key === 'string' && typeof atMs === 'number';
}

/**
 * A preview frame as the app's channel sends it (`recorder_previews`; `frames::frame_message` in the app): the
 * header's length in two bytes (big-endian), the header (`PreviewFrame`) as UTF-8 JSON, then the JPEG's own bytes, as
 * a `Blob` (no base64, no data URL). Null when the message isn't one: too short, or a header that doesn't read.
 */
export function previewFrame(data: ArrayBuffer | ArrayBufferView): PreviewMessage | null {
  const bytes =
    data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (bytes.length < 2) return null;
  const n = (bytes[0] << 8) | bytes[1];
  if (bytes.length <= 2 + n) return null;
  let frame: unknown;
  try {
    frame = JSON.parse(text.decode(bytes.subarray(2, 2 + n)));
  } catch {
    return null;
  }
  if (!isFrame(frame)) return null;
  // A copy, as the Blob makes anyway: a view's buffer may be shared.
  return { frame, jpeg: new Blob([bytes.slice(2 + n)], { type: 'image/jpeg' }) };
}
