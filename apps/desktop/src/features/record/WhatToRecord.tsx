import { type KeyboardEvent, useId } from 'react';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { Segmented } from '../../design/Segmented';
import type { Picks } from '../../lib/recorder/choices';
import { useRecorder } from '../../state/recorder';
import { useRecorderPreviews } from '../../state/recorderPreviews';
import { NO_SCREEN } from './notes';
import { ThumbCard } from './ThumbCard';

/** The cards' group: a press on Record with nothing chosen puts the keyboard on it. */
export const CARDS_ID = 'record-cards';

/** The arrow keys' step in the card group: a radio group moves to the previous or next card, and wraps. */
const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 };

interface Card {
  value: string;
  /** Its stills' key (`display:<id>`, `app:<appId>`). */
  key: string;
  name: string;
  meta: string | null;
}

/** The cards for the kind chosen: each display with its size, or each app with windows. */
function cardsOf(setup: RecorderSetup, picks: Picks): Card[] {
  return picks.kind === 'screen'
    ? setup.devices.displays.map((d) => ({
        value: d.id,
        key: `display:${d.id}`,
        name: d.name,
        meta: `${d.width} x ${d.height}`,
      }))
    : // The setup doesn't count an app's windows: its card has no meta line.
      setup.devices.apps.map((a) => ({ value: a.appId, key: `app:${a.appId}`, name: a.name, meta: null }));
}

/**
 * "What to record": Whole screen or One app, then a card per screen or per app, a radio group whose arrow
 * keys move and choose. With One app and none chosen, the group says to choose one.
 */
export function WhatToRecord({ setup }: { setup: RecorderSetup }) {
  const picks = useRecorder((s) => s.picks);
  const pick = useRecorder((s) => s.pick);
  const stills = useRecorderPreviews((s) => s.thumbs);
  const stillsAt = useRecorderPreviews((s) => s.thumbsAt);
  const headingId = useId();
  const captionId = useId();
  const screens = picks.kind === 'screen';
  const cards = cardsOf(setup, picks);
  const chosen = screens ? picks.display : picks.appId;
  const choose = (value: string) => pick(screens ? { display: value } : { appId: value });
  // The lists are read as Record opens: a screen or an app that comes later needs a new opening.
  const caption =
    chosen !== null
      ? null
      : screens
        ? NO_SCREEN
        : cards.length === 0
          ? 'No apps with windows are open. Open the app, then open Record again.'
          : 'Choose an app';
  const stop = cards.some((c) => c.value === chosen) ? chosen : (cards[0]?.value ?? null);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = STEPS[e.key];
    if (step === undefined || cards.length === 0) return;
    e.preventDefault();
    const at = cards.findIndex((c) => c.value === chosen);
    const from = at < 0 ? (step > 0 ? -1 : 0) : at;
    const next = cards[(from + step + cards.length) % cards.length];
    choose(next.value);
    e.currentTarget.querySelector<HTMLElement>(`[data-value="${CSS.escape(next.value)}"]`)?.focus();
  };

  return (
    <section className="record-section" aria-labelledby={headingId}>
      <h3 className="record-label" id={headingId}>
        What to record
      </h3>
      <Segmented
        label="What to record"
        value={picks.kind}
        options={[
          { value: 'screen', label: 'Whole screen' },
          { value: 'app', label: 'One app' },
        ]}
        onChange={(kind) => pick({ kind })}
      />
      {caption !== null && (
        <p className="record-caption" id={captionId}>
          {caption}
        </p>
      )}
      <div
        className="record-cards"
        id={CARDS_ID}
        role="radiogroup"
        aria-label={screens ? 'Screens' : 'Apps'}
        aria-describedby={caption === null ? undefined : captionId}
        onKeyDown={onKeyDown}
      >
        {cards.map((card) => (
          <ThumbCard
            key={card.value}
            value={card.value}
            name={card.name}
            meta={card.meta}
            picture={stills[card.key] ?? null}
            pictureAt={stillsAt[card.key] ?? null}
            kind={picks.kind}
            checked={card.value === chosen}
            tabbable={card.value === stop}
            onPick={() => choose(card.value)}
          />
        ))}
      </div>
    </section>
  );
}
