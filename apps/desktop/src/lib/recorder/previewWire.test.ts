import { describe, expect, it } from 'vitest';
import { frameMessage } from '../../test/recorderPreviews';
import type { PreviewFrame } from '../../bindings/PreviewFrame';
import { previewFrame, previewKind } from './previewWire';

describe('previewFrame', () => {
  it('reads_the_header_then_the_jpeg_s_own_bytes_as_a_blob', async () => {
    const jpeg = [0xff, 0xd8, 0x00, 0x7f, 0xff, 0xd9];
    const head = new TextEncoder().encode(JSON.stringify({ kind: 'live', key: 'display:1', atMs: 1_700_000_000_000 }));
    const message = new Uint8Array([0, head.length, ...head, ...jpeg]);
    const read = previewFrame(message.buffer);
    expect(read?.frame).toEqual({ kind: 'live', key: 'display:1', atMs: 1_700_000_000_000 });
    expect(read?.jpeg.type).toBe('image/jpeg');
    expect([...new Uint8Array(await (read?.jpeg ?? new Blob()).arrayBuffer())]).toEqual(jpeg);
  });

  it('reads_a_header_longer_than_255_bytes', async () => {
    const key = `app:${'x'.repeat(400)}`;
    const read = previewFrame(frameMessage({ kind: 'thumb', key, atMs: 5 }, 'still'));
    expect(read?.frame.key).toBe(key);
    expect(await read?.jpeg.text()).toBe('still');
  });

  it('is_null_for_what_is_not_a_frame', () => {
    expect(previewFrame(new ArrayBuffer(0))).toBeNull();
    expect(previewFrame(new Uint8Array([0]).buffer)).toBeNull();
    // A header with nothing after it.
    const head = new TextEncoder().encode('{"kind":"thumb","key":"display:1","atMs":1}');
    expect(previewFrame(new Uint8Array([0, head.length, ...head]).buffer)).toBeNull();
    // A header that isn't JSON, or not a frame's.
    expect(previewFrame(new Uint8Array([0, 2, 0x7b, 0x7b, 0xff]).buffer)).toBeNull();
    expect(previewFrame(frameMessage({ kind: 'screen' as 'thumb', key: 'display:1', atMs: 1 }, 'x'))).toBeNull();
  });

  it('names_the_kind_of_a_frame_that_does_not_read_when_its_header_does', () => {
    expect(previewKind(frameMessage({ kind: 'camera', key: 'cam' } as PreviewFrame, 'x'))).toBe('camera');
    expect(previewKind(frameMessage({ kind: 'thumb', key: 'display:1', atMs: 1 }, ''))).toBe('thumb');
    expect(previewKind(frameMessage({ kind: 'screen', key: 'display:1', atMs: 1 } as unknown as PreviewFrame, 'x'))).toBeNull();
    expect(previewKind(new Uint8Array([0, 2, 0x7b, 0x7b, 0xff]).buffer)).toBeNull();
    expect(previewKind(new ArrayBuffer(1))).toBeNull();
  });
});
