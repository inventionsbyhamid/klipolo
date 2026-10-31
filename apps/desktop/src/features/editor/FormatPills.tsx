import type { KeyboardEvent } from 'react';
import { FORMAT_SHORT } from '../../lib/layout';
import { FORMAT_ORDER, switchFormat, useShownFormat } from './formats';

/**
 * The preview's format switch: 16:9, 9:16 and 1:1 as a segmented radio group, the format shown checked.
 * A pick switches the project's format (`switchFormat`, one undo step the top bar names); the arrow keys move along.
 */
export function FormatPills() {
  const shown = useShownFormat();
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const step = e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 1;
    const at = FORMAT_ORDER.indexOf(shown);
    const next = FORMAT_ORDER[(at + step + FORMAT_ORDER.length) % FORMAT_ORDER.length];
    void switchFormat(next);
    e.currentTarget.querySelector<HTMLElement>(`[data-format="${next}"]`)?.focus();
  };
  return (
    <div className="seg" role="radiogroup" aria-label="Format" onKeyDown={onKeyDown}>
      {FORMAT_ORDER.map((format) => (
        <button
          key={format}
          type="button"
          role="radio"
          aria-checked={shown === format}
          tabIndex={shown === format ? 0 : -1}
          data-format={format}
          title={`${FORMAT_SHORT[format]} (${FORMAT_ORDER.indexOf(format) + 1})`}
          onClick={() => void switchFormat(format)}
        >
          {FORMAT_SHORT[format]}
        </button>
      ))}
    </div>
  );
}
