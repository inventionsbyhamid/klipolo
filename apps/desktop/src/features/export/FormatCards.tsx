import type { FormatKind } from '../../bindings/FormatKind';
import type { PlannedFile } from '../../bindings/PlannedFile';
import { cn } from '../../design/cn';
import { Icon } from '../../layout/Icon';
import { FORMAT_SHORT } from '../../lib/layout';
import { FORMATS, formatSize } from './presets';

interface FormatCardsProps {
  checked: FormatKind[];
  /** The files the plan (or the run) has, for their exact sizes. */
  files: PlannedFile[];
  /** While exporting the cards show what it makes and can't be changed. */
  locked: boolean;
  onToggle: (format: FormatKind) => void;
}

/**
 * The format cards: one per format, the frame drawn to scale with its name and pixel size, checked with a
 * paper ring. Each is a checkbox; while an export runs they are locked (55% opacity, not clickable).
 */
export function FormatCards({ checked, files, locked, onToggle }: FormatCardsProps) {
  return (
    <div>
      <p className="ex-label" id="export-formats">
        Formats
      </p>
      <div className="ex-formats" role="group" aria-labelledby="export-formats">
        {FORMATS.map((format) => {
          const [width, height] = formatSize(format, files.find((f) => f.format === format));
          const on = checked.includes(format);
          return (
            <button
              key={format}
              type="button"
              // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
              tabIndex={0}
              role="checkbox"
              aria-checked={on}
              aria-disabled={locked || undefined}
              aria-label={`${FORMAT_SHORT[format]}, ${width} x ${height}`}
              className={cn('ex-fmt', locked && 'is-locked')}
              onClick={() => !locked && onToggle(format)}
            >
              <span className="ex-box" aria-hidden>
                <Icon name="check" />
              </span>
              <span className="ex-shape" aria-hidden>
                <i className={`ex-frame-${format}`} />
              </span>
              <span className="ex-name">{FORMAT_SHORT[format]}</span>
              <span className="ex-meta">
                {width} x {height}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
