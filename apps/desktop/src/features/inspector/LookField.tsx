import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import { Tooltip } from '../../design/Tooltip';
import { showLeftView } from '../../state/libraryPanel';
import { presetName } from '../looks/looks';
import { Field } from './fields';

/**
 * A clip's look (the clip card): its name, on a button that opens the left column's Looks view, where looks are
 * picked. The card steps aside for it (the clip stays selected), and a folded library opens.
 */
export function LookField({ clip }: { clip: Clip }) {
  const { t } = useLingui();
  const label = t({ id: 'clip.look', message: 'Look', comment: 'A clip’s colour look' });
  const name = presetName(clip.look.preset);
  return (
    <Field label={label}>
      <Tooltip label={t({ id: 'clip.look.tip', message: 'Choose a look in the Looks panel' })}>
        <button
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="select"
          aria-label={`${label}: ${name}`}
          onClick={() => showLeftView('looks')}
        >
          <span className="select-text">{name}</span>
        </button>
      </Tooltip>
    </Field>
  );
}
