import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef } from 'react';
import { useEditFrames, type FileFrame } from '../../state/editFrames';
import { useMedia } from '../../state/media';
import { chunkFiles } from './media/drawFilmstrip';
import { spriteCache, stripOf } from './media/spriteCache';

/** How wide (CSS px) each of the two frames draws. */
const FRAME_PX = 168;

/** One frame from its file's filmstrip: the tile of the second it is in, drawn when its sprite has decoded. */
function EdgeFrame({ frame, label }: { frame: FileFrame | null; label: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const version = useMedia((s) => (frame ? (s.versions[frame.path] ?? 0) : 0));
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas || !frame) return;
    let live = true;
    void (async () => {
      const strip = await stripOf(frame.path, version).catch(() => null);
      if (!live || !strip || !(strip.slotUs > 0)) return;
      const slot = Math.floor(frame.fileUs / strip.slotUs);
      const sprite = chunkFiles(strip).get(Math.floor(slot / strip.perChunk));
      if (!sprite) return;
      const img = await spriteCache.load(sprite, () => live).catch(() => null);
      const ctx = live && img ? canvas.getContext('2d') : null;
      if (!ctx || !img) return;
      const cell = slot % strip.perChunk;
      const [col, row] = [cell % strip.cols, Math.floor(cell / strip.cols)];
      canvas.width = FRAME_PX * devicePixelRatio;
      canvas.height = Math.round((FRAME_PX * strip.tileH * devicePixelRatio) / strip.tileW);
      ctx.drawImage(
        img,
        col * strip.tileW,
        row * strip.tileH,
        strip.tileW,
        strip.tileH,
        0,
        0,
        canvas.width,
        canvas.height,
      );
    })();
    return () => {
      live = false;
    };
  }, [frame, version]);
  return (
    <figure className="edit-frame">
      <canvas ref={ref} width={FRAME_PX} height={Math.round((FRAME_PX * 9) / 16)} />
      <figcaption>{label}</figcaption>
    </figure>
  );
}

/**
 * While a roll, slip or slide is dragged, the two frames that meet at the edit, side by side over the bottom of the
 * preview (the preview itself shows the edit's frame at full size): the outgoing clip's last and the incoming clip's
 * first; a slip's first and last. Drawn from the clips' filmstrips, so nothing new is decoded. Under them, the edit's
 * change in time so far.
 */
export function EditFrames() {
  const { t } = useLingui();
  const pair = useEditFrames((s) => s.pair);
  const ends = useEditFrames((s) => s.ends);
  const change = useEditFrames((s) => s.change);
  if (!pair) return null;
  const [left, right] = ends
    ? [
        t({ id: 'preview.editFrames.first', message: 'First frame', comment: "A slipped clip's first frame" }),
        t({ id: 'preview.editFrames.last', message: 'Last frame', comment: "A slipped clip's last frame" }),
      ]
    : [
        t({ id: 'preview.editFrames.out', message: 'Out', comment: 'The last frame before a cut' }),
        t({ id: 'preview.editFrames.in', message: 'In', comment: 'The first frame after a cut' }),
      ];
  return (
    <div className="edit-frames" aria-hidden data-testid="edit-frames">
      <EdgeFrame frame={pair[0]} label={left} />
      <EdgeFrame frame={pair[1]} label={right} />
      {change && <span className="edit-frames-change">{change}</span>}
    </div>
  );
}
