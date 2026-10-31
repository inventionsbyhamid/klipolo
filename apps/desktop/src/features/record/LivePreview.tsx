import { useEffect, useRef, useState } from 'react';
import { useRecorderPreviews } from '../../state/recorderPreviews';

/**
 * A picture this long without a newer one is shown grey: a card gets a still about once a second (about 2 s on a slow
 * Mac), so one older than this has stopped coming, though its preview may still run.
 */
export const STALE_MS = 3_500;

/** Whether a picture that came at `at` (ms since 1970, its frame's `atMs`) is `STALE_MS` old, as time goes by. */
export function useStale(at: number | null): boolean {
  const [stale, setStale] = useState(false);
  useEffect(() => {
    if (at === null) {
      setStale(false);
      return;
    }
    const left = at + STALE_MS - Date.now();
    setStale(left <= 0);
    if (left <= 0) return;
    const timer = setTimeout(() => setStale(true), left);
    return () => clearTimeout(timer);
  }, [at]);
  return stale;
}

/**
 * A picture that changes many times a second, from object URLs, without flicker. Each new frame decodes in the picture
 * behind and comes forward only once it is ready: changing the `src` of the picture on screen lets WebKit paint the
 * box empty until the new frame is decoded. A frame that can't be decoded (let go meanwhile) is skipped and the last
 * one stays. Nothing shows until the first frame (the box's own colour, or what is under it). Its box carries the
 * name for screen readers; the two pictures are left out.
 */
export function LivePicture({ url }: { url: string | null }) {
  const first = useRef<HTMLImageElement>(null);
  const second = useRef<HTMLImageElement>(null);
  /** The picture on screen, null before the first frame. */
  const front = useRef<HTMLImageElement | null>(null);

  useEffect(() => {
    const a = first.current;
    const b = second.current;
    if (!a || !b) return;
    if (url === null) {
      for (const img of [a, b]) {
        img.style.visibility = '';
        img.removeAttribute('src');
      }
      front.current = null;
      return;
    }
    const back = front.current === a ? b : a;
    let gone = false;
    back.src = url;
    const ready = typeof back.decode === 'function' ? back.decode() : Promise.resolve();
    void ready.then(
      () => {
        if (gone) return;
        const was = front.current;
        back.style.visibility = 'visible';
        if (was !== null && was !== back) {
          // The frame it replaced goes, so a still picture keeps one decoded frame.
          was.style.visibility = '';
          was.removeAttribute('src');
        }
        front.current = back;
      },
      () => undefined,
    );
    return () => {
      gone = true;
    };
  }, [url]);

  return (
    <span className="live-pic" aria-hidden>
      <img ref={first} alt="" />
      <img ref={second} alt="" />
    </span>
  );
}

/**
 * The live picture of what Record records: the chosen screen or app at about 10 frames a second, fitted in
 * the preview box over black. Nothing until its first frame; grey once its frames stop coming. `name`: the screen's
 * or the app's, as its card says.
 */
export function LivePreview({ name }: { name: string }) {
  const url = useRecorderPreviews((s) => s.live);
  const at = useRecorderPreviews((s) => s.liveAt);
  const stale = useStale(at);
  if (url === null) return null;
  return (
    <div className="record-live" role="img" aria-label={`Live view of ${name}`} data-stale={stale || undefined}>
      <LivePicture url={url} />
    </div>
  );
}
