import { useLingui } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { FillFocus } from '../../bindings/FillFocus';
import type { FormatKind } from '../../bindings/FormatKind';
import type { SpeakerEstimate } from '../../bindings/SpeakerEstimate';
import { speakerBridge } from '../../bridge/speaker';
import { Segmented } from '../../design/Segmented';
import { editClip } from '../../state/latest';
import { useProject } from '../../state/project';
import { about } from '../analysis/analysis';
import { Field } from './fields';
import { followSpeaker, useSpeakerRun } from './speakerRuns';

/** Where a Fill crop keeps its focus in a format. */
export type FocusChoice = 'speaker' | 'fixed' | 'center';

/** The choice `clip` has in `format`: its own focus there, else Fixed (its framing's focus point). */
export function focusChoice(clip: Clip, format: FormatKind): FocusChoice {
  const focus = clip.fillFocus?.[format];
  return focus?.kind === 'speaker' ? 'speaker' : focus?.kind === 'center' ? 'center' : 'fixed';
}

/** `clip`'s focus in every format with `format`'s set to `focus` (none for Fixed). */
export function focusPatch(clip: Clip, format: FormatKind, focus: FillFocus | null) {
  const fillFocus = { ...clip.fillFocus };
  if (focus) fillFocus[format] = focus;
  else delete fillFocus[format];
  return { fillFocus };
}

/**
 * What following the speaker in `clip` costs in `format`, asked again as the project changes (tracked footage is free);
 * null until Rust answers or when the clip can't follow.
 */
function useSpeakerEstimate(clipId: string, format: FormatKind, can: boolean): SpeakerEstimate | null {
  const revision = useProject((s) => s.snapshot?.history.revision ?? -1);
  const [answer, setAnswer] = useState<{ key: string; estimate: SpeakerEstimate } | null>(null);
  const key = `${clipId}:${format}`;
  useEffect(() => {
    if (!can) return;
    let live = true;
    speakerBridge.estimate(clipId, format).then(
      (estimate) => {
        if (live) setAnswer({ key, estimate });
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [clipId, format, can, key, revision]);
  return can && answer?.key === key ? answer.estimate : null;
}

/**
 * A Fill crop's focus in the format shown (one row while Fill crops): Speaker follows the person talking, Fixed keeps
 * the Fill focus the slider below sets, Centre keeps the middle. Speaker is offered for footage that can follow (an
 * imported video or a camera take: `canFollow`), with its price said first ("Follow the speaker, about $0.02") and its
 * progress while Rust tracks. Fixed and Centre are one undo step each; Speaker lands as one when the tracking is done.
 */
export function FocusField({ clip, format, canFollow }: { clip: Clip; format: FormatKind; canFollow: boolean }) {
  const { t } = useLingui();
  const choice = focusChoice(clip, format);
  const estimate = useSpeakerEstimate(clip.id, format, canFollow);
  const running = useSpeakerRun(clip.id, format);
  const pick = (next: FocusChoice) => {
    if (next === choice || running !== null) return;
    // Refused (no key or code, more than an access code's credit left): the note under the row says why.
    if (next === 'speaker') return void (estimate?.refusal || followSpeaker(clip.id, format));
    void editClip(next === 'center' ? 'clip.focus.center.undo' : 'clip.focus.fixed.undo', clip, (now) =>
      focusPatch(now, format, next === 'center' ? { kind: 'center' } : null),
    );
  };
  const label = t({ id: 'clip.focusChoice', message: 'Focus', comment: 'Where a filled video keeps its focus' });
  const speakerOption = {
    value: 'speaker' as const,
    label: t({ id: 'clip.focusChoice.speaker', message: 'Speaker', comment: 'Focus: follow the person talking' }),
    tooltip: t({ id: 'clip.focusChoice.speaker.tip', message: 'Follow the speaker' }),
  };
  const options = [
    ...(canFollow ? [speakerOption] : []),
    {
      value: 'fixed' as const,
      label: t({ id: 'clip.focusChoice.fixed', message: 'Fixed', comment: 'Focus: stay where it is set' }),
      tooltip: t({ id: 'clip.focusChoice.fixed.tip', message: 'Keep the focus where you set it' }),
    },
    {
      value: 'center' as const,
      label: t({ id: 'clip.focusChoice.center', message: 'Centre', comment: 'Focus: keep the middle in view' }),
      tooltip: t({ id: 'clip.focusChoice.center.tip', message: 'Keep the middle in view' }),
    },
  ];
  const note =
    running !== null
      ? t({ id: 'clip.focusChoice.running', message: `Finding the speaker, ${running}%` })
      : choice === 'speaker' || !canFollow || !estimate
        ? null
        : (estimate.refusal ??
          (estimate.cached
            ? t({ id: 'clip.focusChoice.free', message: 'Follow the speaker, no charge' })
            : t({ id: 'clip.focusChoice.price', message: `Follow the speaker, ${about(estimate.usd)}` })));
  return (
    <>
      <Field label={label}>
        <Segmented
          label={label}
          value={running !== null ? 'speaker' : choice}
          options={options}
          fill
          disabled={running !== null}
          onChange={pick}
        />
      </Field>
      {note && (
        <p className="field-note" role={running !== null ? 'status' : undefined}>
          {note}
        </p>
      )}
    </>
  );
}
