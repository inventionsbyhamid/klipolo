import { useLingui } from '@lingui/react/macro';
import type { ChatMoment } from '../../bindings/ChatMoment';
import { Icon } from '../../layout/Icon';
import { formatPrecise } from '../../lib/time';
import { useChat } from '../../state/chat';
import { usePlayback } from '../../state/playback';

/**
 * The moments a reply points at: a pill row each with a round play disc, the time in faint and the
 * title; a click moves the playhead there and plays. "Keep only these" sends a keep-only request with their ranges,
 * which Klipolo proposes like any other removal, on `revision`, the reply's (their times are on it). `stale`: the
 * project changed since the reply, so their times may have moved: the chips still seek, but Keep only these is off and
 * says why.
 */
export function Moments({ moments, revision, stale }: { moments: ChatMoment[]; revision: number; stale: boolean }) {
  const { t } = useLingui();
  if (moments.length === 0) return null;
  return (
    <>
      <div className="moments">
        {moments.map((m, i) => (
          <button
            key={i}
            type="button"
            className="moment"
            aria-label={t({
              id: 'chatPanel.moment',
              message: `${formatPrecise(m.startUs)} ${m.label}`,
              comment: 'A moment chat points at: where it starts, then its title',
            })}
            onClick={() => {
              const playback = usePlayback.getState();
              playback.seek(m.startUs);
              // From the moment, even while Loop plays a time range elsewhere.
              playback.play(1, { here: true });
            }}
          >
            <span className="mplay" aria-hidden>
              <Icon name="play" />
            </span>
            <span className="when">{formatPrecise(m.startUs)}</span>
            <span className="what">{m.label}</span>
          </button>
        ))}
      </div>
      <div className="msg-row">
        <button
          type="button"
          className="chip chip-outline"
          disabled={stale}
          onClick={() =>
            void useChat.getState().send(t({ id: 'chatPanel.keepOnly', message: 'Keep only these' }), {
              keepOnly: moments.map((m) => ({ start: m.startUs, end: m.endUs })),
              baseRevision: revision,
            })
          }
        >
          {t({ id: 'chatPanel.keepOnly', message: 'Keep only these' })}
        </button>
      </div>
      {stale && (
        <p className="muted">
          {t({ id: 'chatPanel.keepOnly.stale', message: 'The video changed since. Ask again to keep only these.' })}
        </p>
      )}
    </>
  );
}
