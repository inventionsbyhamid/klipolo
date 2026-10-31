import { useLingui } from '@lingui/react/macro';
import { SpriteIcon } from '../../design/SpriteIcon';
import { findItem, itemLabel } from '../../lib/timeline';
import { formatPrecise } from '../../lib/time';
import { useProject } from '../../state/project';
import { useRange } from '../../state/range';
import { useSelection } from '../../state/selection';

/**
 * What "this" means in the next message, above the chat box: the time range selected on the timeline
 * ("Selection 4:26.2 to 4:31.8"), else the overlay selected ("Title: A day at Rosa's studio"), with an x that clears
 * it. Nothing when nothing is selected.
 */
export function SelectionChip() {
  const { t } = useLingui();
  const range = useRange((s) => s.range);
  const selectedId = useSelection((s) => s.selectedId);
  const project = useProject((s) => s.snapshot?.project);
  if (range) {
    const from = formatPrecise(range.startUs);
    const to = formatPrecise(range.endUs);
    return (
      <div className="sel-chip">
        <SpriteIcon name="range" />
        <span>{t({ id: 'chatPanel.selection.range', message: `Selection ${from} to ${to}` })}</span>
        <button
          type="button"
          className="x"
          aria-label={t({ id: 'chatPanel.selection.clear', message: 'Clear selection' })}
          onClick={() => useRange.getState().clear()}
        >
          <SpriteIcon name="x" />
        </button>
      </div>
    );
  }
  const found = project && selectedId ? findItem(project, selectedId) : null;
  if (!found || !project) return null;
  const text = found.item.content.type === 'text';
  const name = itemLabel(project, found.item);
  return (
    <div className="sel-chip">
      <SpriteIcon name="text" />
      <span>
        {text ? t({ id: 'chatPanel.selection.title', message: `Title: ${name}` }) : name}
      </span>
      <button
        type="button"
        className="x"
        aria-label={t({ id: 'chatPanel.selection.clear', message: 'Clear selection' })}
        onClick={() => useSelection.getState().select(null)}
      >
        <SpriteIcon name="x" />
      </button>
    </div>
  );
}
