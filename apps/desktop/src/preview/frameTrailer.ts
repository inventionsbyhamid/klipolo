/** The 56-byte little-endian trailer after each frame's pixels. Mirrors `klipolo_engine::frames::trailer`. */
export const TRAILER_LEN = 56;
/** `MAGIC` in the engine's trailer (four ASCII bytes read as a little-endian u32). */
const MAGIC = 0x32464c4b;
/** RGBA, 8 bits per channel: `stride × height` bytes. */
export const FORMAT_RGBA8 = 0;
/** NV12 (BT.709 limited range): a `stride × height` Y plane, then a `stride × height / 2` interleaved UV plane. */
export const FORMAT_NV12 = 1;
export const FLAG_PLAYBACK = 1;

export interface FrameMeta {
  width: number;
  height: number;
  stride: number;
  format: number;
  flags: number;
  seq: number;
  session: number;
  revision: number;
  timeUs: number;
  sentMs: number;
  /** When it should be on screen (ms since the Unix epoch); 0: as soon as possible (stills). */
  dueMs: number;
}

/** Bytes of pixels a frame holds. */
export function pixelBytes(format: number, stride: number, height: number): number {
  return format === FORMAT_NV12 ? stride * height + stride * Math.ceil(height / 2) : stride * height;
}

/** The frame's metadata, or null when the buffer isn't a frame this UI can draw. */
export function readTrailer(buf: ArrayBuffer): FrameMeta | null {
  if (buf.byteLength < TRAILER_LEN) return null;
  const v = new DataView(buf, buf.byteLength - TRAILER_LEN, TRAILER_LEN);
  const format = v.getUint16(16, true);
  if (v.getUint32(0, true) !== MAGIC || (format !== FORMAT_RGBA8 && format !== FORMAT_NV12)) return null;
  const meta: FrameMeta = {
    width: v.getUint32(4, true),
    height: v.getUint32(8, true),
    stride: v.getUint32(12, true),
    format,
    flags: v.getUint16(18, true),
    seq: v.getUint32(20, true),
    session: v.getUint32(24, true),
    revision: v.getUint32(28, true),
    timeUs: Number(v.getBigInt64(32, true)),
    sentMs: v.getFloat64(40, true),
    dueMs: v.getFloat64(48, true),
  };
  const row = format === FORMAT_NV12 ? 1 : 4;
  if (
    meta.stride < meta.width * row ||
    pixelBytes(format, meta.stride, meta.height) > buf.byteLength - TRAILER_LEN
  )
    return null;
  return meta;
}
