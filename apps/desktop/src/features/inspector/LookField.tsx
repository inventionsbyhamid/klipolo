import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import { Tooltip } from '../../design/Tooltip';
import { useCaptionsView } from '../../state/captionsView';
import { presetName } from '../looks/looks';
import { Field } from './fields';

/** A clip's look (the clip card): its name, on a button that opens the left panel's Looks view, where looks are picked. */
export function LookField({ clip }: { clip: Clip }) {
  const { t } = useLingui();
  const label = t({ id: 'clip.look', message: 'Look', comment: 'A clip’s colour look' });
  const name = presetName(clip.look.preset);
  return (
    <Field label={label}>
      <Tooltip label={t({ id: 'clip.look.tip', message: 'Choose a look in the Looks panel' })}>
        <button
          type="button"
          className="select"
          aria-label={`${label}: ${name}`}
          onClick={() => useCaptionsView.getState().setView('looks')}
        >
          <span className="select-text">{name}</span>
        </button>
      </Tooltip>
    </Field>
  );
}
