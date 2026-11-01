import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef } from 'react';
import type { WordView } from '../../bindings/WordView';
import { TEXT_SWATCHES } from '../../lib/swatches';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { ColorField } from '../color/ColorField';
import { currentLook, lookOps } from './captionOps';
import { lookedNow } from './usePressQueue';

/**
 * The picked words' colour: a compact color field. Its value is their colour when they share one; they differ
 * otherwise, and it reads "Mixed". "Style's color" puts the style's own back. A hovered colour is dry-run on the words
 * (not while they are being updated: it would show on words about to change); a pick is one press, "Style caption
 * words", through the press queue, so a pick made while the words update waits for them.
 */
export function WordColor({
  words,
  busy,
  press,
}: {
  words: WordView[];
  busy: boolean;
  press: (change: { color: string | undefined }) => Promise<boolean> | null;
}) {
  const { t } = useLingui();
  /** A colour is being dry-run on the video. */
  const previewing = useRef(false);
  const endPreview = () => {
    if (!previewing.current) return;
    previewing.current = false;
    clearPreview();
  };
  // Gone mid-preview (the words were let go): put the video back.
  useEffect(
    () => () => {
      if (previewing.current) clearPreview();
    },
    [],
  );
  const colors = words.map((w) => currentLook(w).color ?? null);
  const alike = colors.every((c) => c?.toUpperCase() === colors[0]?.toUpperCase());
  const color = colors[0] ?? null;
  return (
    <ColorField
      compact
      label={t({ id: 'captions.words.color', message: 'Word color', comment: 'The color of the caption words picked' })}
      value={alike ? color : null}
      mixed={!alike}
      presets={TEXT_SWATCHES.map((s) => ({ value: s.value, name: t(s.name) }))}
      inherit={{
        label: t({ id: 'captions.color.style', message: 'Style’s color', comment: 'The caption style’s own color' }),
      }}
      onPreview={(c) => {
        // While the words are being updated, a dry run would show on words that are about to change.
        if (busy) return endPreview();
        previewing.current = true;
        previewOps(() => lookOps(lookedNow(words), { color: c ?? undefined }));
      }}
      onPreviewEnd={endPreview}
      onCommit={(c) => {
        // Kept until the words settle, it may never go, so the video goes back.
        if (busy) {
          endPreview();
          press({ color: c ?? undefined });
          return;
        }
        // Going now, its new revision replaces the dry run (no clear: the frame never flickers back). Kept after all,
        // changing nothing or refused, it puts the video back, unless a new dry run began.
        const dryRun = previewing.current;
        previewing.current = false;
        if (dryRun) dropPendingPreview();
        const landed = press({ color: c ?? undefined });
        if (dryRun)
          void (landed ?? Promise.resolve(false)).then((ok) => {
            if (!ok && !previewing.current) clearPreview();
          });
      }}
    />
  );
}
