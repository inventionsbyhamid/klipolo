import { openSettings, useAi } from '../ai/aiStatus';
import { AiIcon } from '../ai/icons';
import { chipView } from './settings';
import './settings.css';

/**
 * The top bar's AI chip: "Add Meta key" with nothing set, the access code's credit left, or the key in use.
 * It opens Settings. Nothing until the status is known.
 */
export function AiChip() {
  const status = useAi((s) => s.status);
  const view = chipView(status);
  if (!view) return null;
  return (
    <button
      type="button"
      className={view.missing ? 'ai-chip is-missing' : 'ai-chip'}
      title="Settings (Command ,)"
      onClick={openSettings}
    >
      <AiIcon name={view.icon} />
      {view.label}
      {view.left && (
        <>
          {' '}
          <b>{view.left}</b> left
        </>
      )}
    </button>
  );
}
