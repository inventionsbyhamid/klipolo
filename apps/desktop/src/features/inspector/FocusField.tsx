import { useLingui } from '@lingui/react/macro';
import { useEffect, useId, useState } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { FillFocus } from '../../bindings/FillFocus';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Keep } from '../../bindings/Keep';
import type { Person } from '../../bindings/Person';
import type { SpeakerEstimate } from '../../bindings/SpeakerEstimate';
import { speakerBridge } from '../../bridge/speaker';
import { Button } from '../../design/Button';
import { Segmented } from '../../design/Segmented';
import { editClip } from '../../state/latest';
import { useProject } from '../../state/project';
import { about } from '../analysis/analysis';
import { Field } from './fields';
import { followSpeaker, keepInFrame, useSpeakerKeep, useSpeakerRun } from './speakerRuns';

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
 * What following the speaker in `clip` costs in `format` on the project as it is now (tracked footage is free); null
 * until Rust answers for this revision, or when the clip can't follow.
 */
function useSpeakerEstimate(clipId: string, format: FormatKind, can: boolean): SpeakerEstimate | null {
  const revision = useProject((s) => s.snapshot?.history.revision ?? -1);
  const [answer, setAnswer] = useState<{ key: string; estimate: SpeakerEstimate } | null>(null);
  // Keyed by the revision too: after an edit the old price is never shown, and Speaker waits for the new one.
  const key = `${clipId}:${format}:${revision}`;
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
  }, [clipId, format, can, key]);
  return can && answer?.key === key ? answer.estimate : null;
}

/**
 * A Fill crop's focus in the format shown (one row while Fill crops): Speaker follows the person talking, Fixed keeps
 * the Fill focus the slider below sets, Centre keeps the middle. Speaker is offered for footage that can follow (an
 * imported video or a camera take: `canFollow`), with its price said first ("Follow the speaker, about $0.02") and its
 * progress while Rust tracks. Fixed and Centre are one undo step each; Speaker lands as one when the tracking is done.
 * Following two or more people, a Keep row below says who stays in frame (`KeepField`).
 */
export function FocusField({ clip, format, canFollow }: { clip: Clip; format: FormatKind; canFollow: boolean }) {
  const { t } = useLingui();
  const choice = focusChoice(clip, format);
  const estimate = useSpeakerEstimate(clip.id, format, canFollow);
  const running = useSpeakerRun(clip.id, format);
  const keeping = useSpeakerKeep(clip.id, format);
  const noteId = useId();
  const refused = choice !== 'speaker' && Boolean(estimate?.refusal);
  // Off until its price has come, so a follow is never paid for unseen.
  const unpriced = choice !== 'speaker' && !estimate;
  const pick = (next: FocusChoice) => {
    if (next === choice || running !== null || keeping) return;
    // Unpriced, or refused (no key or code, more than an access code's credit left): the note under the row says why.
    if (next === 'speaker') return void (!estimate || estimate.refusal || followSpeaker(clip.id, format));
    void editClip(next === 'center' ? 'clip.focus.center.undo' : 'clip.focus.fixed.undo', clip, (now) =>
      focusPatch(now, format, next === 'center' ? { kind: 'center' } : null),
    );
  };
  const label = t({ id: 'clip.focusChoice', message: 'Focus', comment: 'Where a filled video keeps its focus' });
  const speakerOption = {
    value: 'speaker' as const,
    label: t({ id: 'clip.focusChoice.speaker', message: 'Speaker', comment: 'Focus: follow the person talking' }),
    tooltip: t({ id: 'clip.focusChoice.speaker.tip', message: 'Follow the speaker' }),
    // Off while it can't be paid for: the note under the row says why.
    disabled: refused || unpriced,
    describedBy: refused ? noteId : undefined,
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
  const focus = clip.fillFocus?.[format];
  const people = focus?.kind === 'speaker' ? (focus.people ?? []) : [];
  return (
    <>
      <Field label={label}>
        <Segmented
          label={label}
          value={running !== null ? 'speaker' : choice}
          options={options}
          fill
          disabled={running !== null || keeping}
          onChange={pick}
        />
      </Field>
      {note && (
        <p id={noteId} className="field-note" role={running !== null ? 'status' : undefined}>
          {note}
        </p>
      )}
      {focus?.kind === 'speaker' && people.length >= 2 && (
        <KeepField
          clipId={clip.id}
          format={format}
          keep={focus.keep ?? { kind: 'speaker' }}
          people={people}
          busy={running !== null || keeping}
        />
      )}
    </>
  );
}

/** Who a following crop keeps in frame, as the Keep row chooses it: `p1` is the person on the left. */
type KeepChoice = 'speaker' | 'both' | `p${number}`;

/** The row's choice for `keep`; null for chat's own ranges, which no option shows. */
function keepChoice(keep: Keep): KeepChoice | null {
  switch (keep.kind) {
    case 'speaker':
      return 'speaker';
    case 'both':
      return 'both';
    case 'person':
      return `p${keep.person}`;
    case 'ranges':
      return null;
  }
}

/**
 * Who a clip that follows two or more people keeps in frame (one row): whoever is speaking, one of them by where they
 * are, or both (all, for three or more). Who chat set over parts of the clip is said as such, with a reset to whoever
 * is speaking. Rust makes each from what the follow tracked, at no charge, as one undo step.
 */
function KeepField({
  clipId,
  format,
  keep,
  people,
  busy,
}: {
  clipId: string;
  format: FormatKind;
  keep: Keep;
  people: Person[];
  busy: boolean;
}) {
  const { t } = useLingui();
  const label = t({ id: 'clip.keep', message: 'Keep', comment: 'Who a video following several people keeps in view' });
  const n = people.length;
  const place = (i: number) => {
    if (i === 0) return t({ id: 'clip.keep.left', message: 'Left', comment: 'Keep: the person on the left' });
    if (i === n - 1) return t({ id: 'clip.keep.right', message: 'Right', comment: 'Keep: the person on the right' });
    if (n === 3) return t({ id: 'clip.keep.middle', message: 'Middle', comment: 'Keep: the person in the middle' });
    const number = i + 1;
    return t({ id: 'clip.keep.nth', message: `Person ${number}`, comment: 'Keep: a person counted from the left' });
  };
  const options = [
    {
      value: 'speaker' as KeepChoice,
      label: t({ id: 'clip.keep.speaker', message: 'Speaking', comment: 'Keep: whoever is speaking' }),
      tooltip: t({ id: 'clip.keep.speaker.tip', message: 'Keep whoever is speaking in frame' }),
    },
    ...people.map((_, i) => ({
      value: `p${i + 1}` as KeepChoice,
      label: place(i),
      tooltip: t({ id: 'clip.keep.person.tip', message: 'Keep this person in frame' }),
    })),
    {
      value: 'both' as KeepChoice,
      label:
        n === 2
          ? t({ id: 'clip.keep.both', message: 'Both', comment: 'Keep: both people' })
          : t({ id: 'clip.keep.all', message: 'All', comment: 'Keep: everyone' }),
      tooltip: t({ id: 'clip.keep.both.tip', message: 'Keep everyone in frame where they fit' }),
    },
  ];
  const choose = (next: KeepChoice) => {
    const keepNext: Keep =
      next === 'speaker'
        ? { kind: 'speaker' }
        : next === 'both'
          ? { kind: 'both' }
          : { kind: 'person', person: Number(next.slice(1)) };
    void keepInFrame(clipId, format, keepNext);
  };
  const custom = keep.kind === 'ranges';
  return (
    <>
      <Field label={label}>
        <Segmented label={label} value={keepChoice(keep)} options={options} fill disabled={busy} onChange={choose} />
      </Field>
      {custom && (
        <div className="field-note keep-custom">
          <span>{t({ id: 'clip.keep.custom', message: 'Custom by chat' })}</span>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => choose('speaker')}>
            {t({ id: 'clip.keep.reset', message: 'Reset', comment: 'Keep whoever is speaking again' })}
          </Button>
        </div>
      )}
    </>
  );
}
