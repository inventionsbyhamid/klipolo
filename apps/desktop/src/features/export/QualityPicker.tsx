import { useLingui } from '@lingui/react/macro';
import { type KeyboardEvent, useId } from 'react';
import { useExport } from '../../state/export';
import { QUALITIES } from './presets';

/**
 * The quality of every file in the export: a segmented radio group, High unless the person picks another,
 * and under it what the picked one is for (Master adds that it is slower where the CPU encodes its ProRes,
 * `slowMaster`). The arrow keys move between the four.
 */
export function QualityPicker({ slowMaster }: { slowMaster: boolean }) {
  const { t } = useLingui();
  const id = useId();
  const quality = useExport((s) => s.quality);
  const setQuality = useExport((s) => s.setQuality);
  const option = QUALITIES.find((q) => q.value === quality) ?? QUALITIES[1];
  const description = t(option.description);
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
    <div className="ex-quality">
      <div className="seg" role="radiogroup" aria-label="Quality" aria-describedby={id} onKeyDown={onKeyDown}>
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
      <p className="ex-quality-note" id={id}>
        {quality === 'master' && slowMaster
          ? t({
              id: 'export.quality.slow',
              message: `${description} Slower on this computer.`,
              comment: '{description} is the quality’s description, e.g. “ProRes for another editor. Very large files.”',
            })
          : description}
      </p>
    </div>
  );
}
