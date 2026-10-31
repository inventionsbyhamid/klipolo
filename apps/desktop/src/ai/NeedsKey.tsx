import { openSettings, useNeedsKey } from './aiStatus';
import { AiIcon } from './icons';
import './ai.css';

/**
 * What every AI surface shows when there is no key and no code: a card that says AI needs one and that
 * editing and export work without it, with a way to Settings. Nothing while a key or code is set, or before the status
 * is known.
 */
export function NeedsKey() {
  const needed = useNeedsKey();
  if (!needed) return null;
  return (
    <div className="card needs-key">
      <span className="needs-key-ico">
        <AiIcon name="key" />
      </span>
      <h3>Add your Meta API key or access code in Settings</h3>
      <p>Chat, transcripts and captions use Meta's models. Editing and export work without them.</p>
      <button type="button" className="btn btn-secondary btn-sm" onClick={openSettings}>
        Open Settings
      </button>
    </div>
  );
}
