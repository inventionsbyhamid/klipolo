import { useLingui } from '@lingui/react/macro';
import { SpriteIcon } from '../../design/SpriteIcon';
import { formatPrecise } from '../../lib/time';
import { useRange } from '../../state/range';

/**
 * What "this" means in the next message, above the chat box: the time range selected on the timeline
 * ("Selection 4:26.2 to 4:31.8"), with an x that clears it. Nothing otherwise: a selected overlay (a title, a picture)
 * gets no chip, since no chat tool changes a layer yet and the model doesn't see it, so "this" would mislead.
 */
export function SelectionChip() {
  const { t } = useLingui();
  const range = useRange((s) => s.range);
  if (!range) return null;
  const from = formatPrecise(range.startUs);
  const to = formatPrecise(range.endUs);
  return (
    <div className="sel-chip">
      <SpriteIcon name="range" />
      <span>{t({ id: 'chatPanel.selection.range', message: `Selection ${from} to ${to}` })}</span>
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className="x"
        aria-label={t({ id: 'chatPanel.selection.clear', message: 'Clear selection' })}
        onClick={() => useRange.getState().clear()}
      >
        <SpriteIcon name="x" />
      </button>
    </div>
  );
}
