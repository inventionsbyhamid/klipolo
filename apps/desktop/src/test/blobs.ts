/** The Blobs object URLs were made from, while they are held (jsdom has no `URL.createObjectURL`). */
const made = new Map<string, Blob>();
let serial = 0;

/** A stand-in JPEG frame for tests: preview frames come as Blobs. */
export const jpeg = (name: string) => new Blob([name], { type: 'image/jpeg' });

/** The Blob an object URL shows, while it is held; undefined once let go. */
export const blobAt = (url: string | null): Blob | undefined => (url === null ? undefined : made.get(url));

/** Object URLs for jsdom (`setup.ts`): each a new `blob:test/N` that remembers its Blob until it is revoked. */
export function installObjectUrls() {
  URL.createObjectURL = (blob: Blob | MediaSource) => {
    const url = `blob:test/${++serial}`;
    made.set(url, blob as Blob);
    return url;
  };
  URL.revokeObjectURL = (url: string) => void made.delete(url);
}
