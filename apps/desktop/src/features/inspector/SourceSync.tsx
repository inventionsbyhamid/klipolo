import { useLingui } from '@lingui/react/macro';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import type { RecordingSource } from '../../bindings/RecordingSource';
import { IconButton } from '../../design/IconButton';
import { useIntlLocale } from '../../i18n/react';
import { formatNumber } from '../../lib/format';
import { isVideoRole } from '../../lib/recording';
import { useProject } from '../../state/project';
import { MAX_NUDGE_US, nudgeSource } from '../timeline/keys/sourceActions';

/** A sync step: 10 ms, or 100 ms with Shift. */
const STEP_US = 10_000;
const BIG_STEP_US = 100_000;

const chevron = (Icon: typeof ChevronLeft) => <Icon size={16} strokeWidth={1.8} aria-hidden />;

/** Whether some of `s` plays from layers of its own (separated): a sync change moves them too. O(layers). */
function useSeparated(s: RecordingSource): boolean {
  return useProject((state) =>
    (state.snapshot?.project.timeline.tracks ?? []).some((t) =>
      t.items.some((i) => {
        const c = i.content;
        return (c.type === 'audio' || c.type === 'video') && s.runs.some((run) => run.assetId === c.assetId);
      }),
    ),
  );
}

/**
 * A recorded source's sync against the rest of its recording, for the whole recording (`updateRecordingSource`'s
 * nudge): how far it is moved, and 10 ms earlier or later, 100 ms with Shift, within the model's 5 s either way. A
 * sound's, or the camera's: a lagging webcam is the commonest sync fault. Each press is one undo step.
 */
export function SourceSync({ recordingId, source, name }: { recordingId: string; source: RecordingSource; name: string }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const separated = useSeparated(source);
  const nudge = source.nudgeUs;
  const ms = formatNumber(locale, Math.abs(nudge) / 1000);
  const sync =
    nudge === 0
      ? t({
          id: 'source.sync.asRecorded',
          message: 'In sync as recorded',
          comment: "A recorded sound's or camera's timing",
        })
      : nudge < 0
        ? t({
            id: 'source.sync.earlier',
            message: `${ms} ms earlier`,
            comment: 'A recorded sound or camera plays this much earlier than it was recorded, to match the rest',
          })
        : t({
            id: 'source.sync.later',
            message: `${ms} ms later`,
            comment: 'A recorded sound or camera plays this much later than it was recorded, to match the rest',
          });
  const earlier = t({
    id: 'source.sync.moveEarlier',
    message: `Move ${name} 10 ms earlier`,
    comment: 'Button: the recorded sound or camera plays a little earlier against the rest; {name} is e.g. Mic',
  });
  const later = t({
    id: 'source.sync.moveLater',
    message: `Move ${name} 10 ms later`,
    comment: 'Button: the recorded sound or camera plays a little later against the rest; {name} is e.g. Mic',
  });
  const shiftTip = t({ id: 'source.sync.shift', message: 'Shift: 100 ms', comment: 'Tooltip: the bigger step' });
  const move = (direction: number, shift: boolean) =>
    nudgeSource(recordingId, source.id, direction * (shift ? BIG_STEP_US : STEP_US));
  return (
    <>
      <div className="sound-sync">
        <span className="sound-sync-text">{sync}</span>
        <IconButton
          className="icon-btn-sm"
          icon={chevron(ChevronLeft)}
          label={earlier}
          tooltip={`${earlier} (${shiftTip})`}
          aria-disabled={nudge <= -MAX_NUDGE_US || undefined}
          onClick={(e) => nudge > -MAX_NUDGE_US && move(-1, e.shiftKey)}
        />
        <IconButton
          className="icon-btn-sm"
          icon={chevron(ChevronRight)}
          label={later}
          tooltip={`${later} (${shiftTip})`}
          aria-disabled={nudge >= MAX_NUDGE_US || undefined}
          onClick={(e) => nudge < MAX_NUDGE_US && move(1, e.shiftKey)}
        />
      </div>
      {separated && (
        <p className="field-note">
          {isVideoRole(source.role)
            ? t({
                id: 'source.sync.separatedCamera',
                message: 'Its separated camera moves with it.',
                comment: "Under the camera's sync buttons, when part of it is on a layer of its own",
              })
            : t({
                id: 'source.sync.separated',
                message: 'Its separated sound moves with it. Its volume and mute stay its own.',
                comment: "Under a recorded sound's sync buttons, when part of it is on a layer of its own",
              })}
        </p>
      )}
    </>
  );
}
