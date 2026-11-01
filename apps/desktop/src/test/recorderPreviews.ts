import { act } from '@testing-library/react';
import type { PreviewFrame } from '../bindings/PreviewFrame';
import type { PreviewKind } from '../bindings/PreviewKind';
import { blobAt } from './blobs';
import type { FakeRecorder } from './recorderBridge';

/**
 * A preview frame as the app sends it over `recorder_previews`'s channel: the header's length in two bytes
 * (big-endian), the header as JSON, then the JPEG's bytes. Its "JPEG" is `name`'s bytes, so a test can tell which frame
 * a picture shows (`nameAt`).
 */
export function frameMessage(header: PreviewFrame, name: string): ArrayBuffer {
  const head = new TextEncoder().encode(JSON.stringify(header));
  const body = new TextEncoder().encode(name);
  const bytes = new Uint8Array(2 + head.length + body.length);
  new DataView(bytes.buffer).setUint16(0, head.length);
  bytes.set(head, 2);
  bytes.set(body, 2 + head.length);
  return bytes.buffer;
}

/**
 * The fake app sends frame `name` of `kind` for `key` (a card's key, or the camera's id) to the page's latest previews
 * call, received at `atMs` (now by default); the pictures it changes are drawn before this resolves.
 */
export async function sendFrame(
  fake: FakeRecorder,
  kind: PreviewKind,
  key: string,
  name: string,
  atMs: number = Date.now(),
): Promise<void> {
  const frames = fake.frames;
  if (frames === null) throw new Error('No previews were asked for');
  await act(async () => frames(frameMessage({ kind, key, atMs }, name)));
}

/** Which frame an object URL shows (the name it was sent with); null for none, or once it was let go. */
export async function nameAt(url: string | null): Promise<string | null> {
  const blob = blobAt(url);
  return blob === undefined ? null : blob.text();
}

/** The object URL a live picture in `el` shows now: its picture in front, null while none is. */
export function shownIn(el: Element | null): string | null {
  const front = [...(el?.querySelectorAll('img') ?? [])].find((img) => img.style.visibility === 'visible');
  return front?.getAttribute('src') ?? null;
}

/** The frame a live picture in `el` shows now, by name (`frameMessage`); null while none is. */
export const frameIn = (el: Element | null) => nameAt(shownIn(el));
