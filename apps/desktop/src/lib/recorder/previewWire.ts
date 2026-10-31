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

const isKind = (kind: unknown): kind is PreviewKind => typeof kind === 'string' && KINDS.has(kind);

/** A message's bytes, and its header's JSON as far as it reads (undefined when it doesn't). */
function readHeader(data: ArrayBuffer | ArrayBufferView): { bytes: Uint8Array; n: number; header: unknown } | null {
  const bytes =
    data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (bytes.length < 2) return null;
  const n = (bytes[0] << 8) | bytes[1];
  try {
    return { bytes, n, header: JSON.parse(text.decode(bytes.subarray(2, 2 + n))) };
  } catch {
    return { bytes, n, header: undefined };
  }
}

function isFrame(value: unknown): value is PreviewFrame {
  if (typeof value !== 'object' || value === null) return false;
  const { kind, key, atMs } = value as Record<string, unknown>;
  return isKind(kind) && typeof key === 'string' && typeof atMs === 'number';
}

/**
 * A preview frame as the app's channel sends it (`recorder_previews`; `frames::frame_message` in the app): the
 * header's length in two bytes (big-endian), the header (`PreviewFrame`) as UTF-8 JSON, then the JPEG's own bytes, as
 * a `Blob` (no base64, no data URL). Null when the message isn't one: too short, or a header that doesn't read.
 */
export function previewFrame(data: ArrayBuffer | ArrayBufferView): PreviewMessage | null {
  const read = readHeader(data);
  if (read === null) return null;
  const { bytes, n, header: frame } = read;
  if (bytes.length <= 2 + n || !isFrame(frame)) return null;
  // A copy, as the Blob makes anyway: a view's buffer may be shared.
  return { frame, jpeg: new Blob([bytes.slice(2 + n)], { type: 'image/jpeg' }) };
}

/**
 * The kind a message's header names, read as far as it reads, null when it names none: a frame that can't be drawn
 * is still acked, so the app never waits on it.
 */
export function previewKind(data: ArrayBuffer | ArrayBufferView): PreviewKind | null {
  const header = readHeader(data)?.header;
  const kind = typeof header === 'object' && header !== null ? (header as { kind?: unknown }).kind : undefined;
  return isKind(kind) ? kind : null;
}
