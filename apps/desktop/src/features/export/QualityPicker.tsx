import { useLingui } from '@lingui/react/macro';
import type { KeyboardEvent } from 'react';
import { useExport } from '../../state/export';
import { QUALITIES } from './presets';

/**
 * The quality of every file in the export: a segmented radio group, High unless the person picks another,
 * file. The arrow keys move between the two.
 */
export function QualityPicker() {
  const { t } = useLingui();
  const quality = useExport((s) => s.quality);
  const setQuality = useExport((s) => s.setQuality);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const at = QUALITIES.findIndex((q) => q.value === quality);
    const step = e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1;
    const next = QUALITIES[(at + step + QUALITIES.length) % QUALITIES.length];
    setQuality(next.value);
    e.currentTarget.querySelector<HTMLElement>(`[data-quality="${next.value}"]`)?.focus();
  };
  return (
    <div className="seg" role="radiogroup" aria-label="Quality" onKeyDown={onKeyDown}>
      {QUALITIES.map((q) => (
        <button
          key={q.value}
          type="button"
          role="radio"
          aria-checked={quality === q.value}
          tabIndex={quality === q.value ? 0 : -1}
          data-quality={q.value}
          onClick={() => setQuality(q.value)}
        >
          {t(q.label)}
        </button>
      ))}
    </div>
  );
}
