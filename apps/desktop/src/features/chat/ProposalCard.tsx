import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { useMemo, useState, type CSSProperties } from 'react';
import type { ChatProposal } from '../../bindings/ChatProposal';
import type { FormatKind } from '../../bindings/FormatKind';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { formatPrecise } from '../../lib/time';
import { useChat, type ReplyMessage } from '../../state/chat';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useProposal } from '../../state/proposal';
import { mainIndex } from '../timeline/model/index';
import { useReturnFocus } from './focus';
import { UndoChip } from './UndoChip';

const FORMAT_NAMES: Record<FormatKind, string> = { landscape: '16:9', portrait: '9:16', square: '1:1' };

/** Whether a key went to the card's Discard button. */
const isDiscard = (target: EventTarget) => target instanceof HTMLElement && target.hasAttribute('data-discard');

/** A length in seconds with one decimal ("2.4 s"). */
const seconds = (us: number) => `${(us / 1_000_000).toFixed(1)} s`;

/** Each removed range's tag ("um", "pause"): the proposal's tag over the most of it. */
function rangeTags(proposal: ChatProposal): string[] {
  return proposal.removals.ranges.map((r) => {
    let [best, most] = ['', 0];
    for (const tag of proposal.tags) {
      const overlap = Math.min(r.end, tag.endUs) - Math.max(r.start, tag.startUs);
      if (overlap > most) [best, most] = [tag.label, overlap];
    }
    return best;
  });
}

/** What a proposal takes away besides footage, one plain sentence each (a format-only one never reads "0 s"). */
function useOtherRemovals(proposal: ChatProposal): string[] {
  const { t } = useLingui();
  const r = proposal.removals;
  const out: string[] = [];
  for (const format of r.formats) {
    const name = FORMAT_NAMES[format];
    out.push(
      t({
        id: 'chatPanel.proposal.format',
        message: `Drops the ${name} version and what was placed for it.`,
        comment: '{name} is a format, e.g. 9:16',
      }),
    );
  }
  if (r.items.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.items',
        message: plural(r.items.length, { one: 'Removes # layer.', other: 'Removes # layers.' }),
      }),
    );
  if (r.shortened.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.shortened',
        message: plural(r.shortened.length, { one: 'Shortens # layer.', other: 'Shortens # layers.' }),
      }),
    );
  // An end card or a closing sound placed after the video moves with its end.
  if (r.moved.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.moved',
        message: plural(r.moved.length, {
          one: 'Moves # layer to the video’s new end.',
          other: 'Moves # layers to the video’s new end.',
        }),
      }),
    );
  if (r.assets.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.assets',
        message: plural(r.assets.length, {
          one: 'Takes # file out of the project.',
          other: 'Takes # files out of the project.',
        }),
      }),
    );
  if (r.tracks.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.tracks',
        message: plural(r.tracks.length, { one: 'Removes # track.', other: 'Removes # tracks.' }),
      }),
    );
  if (r.recordings.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.recordings',
        message: plural(r.recordings.length, { one: 'Removes # recording.', other: 'Removes # recordings.' }),
      }),
    );
  if (r.sounds.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.sounds',
        message: plural(r.sounds.length, {
          one: 'Leaves # recording with less of its sound.',
          other: 'Leaves # recordings with less of their sound.',
        }),
      }),
    );
  if (r.markers.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.markers',
        message: plural(r.markers.length, { one: 'Removes or moves # marker.', other: 'Removes or moves # markers.' }),
      }),
    );
  if (r.luts.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.luts',
        message: plural(r.luts.length, {
          one: 'Takes # color look out of the project.',
          other: 'Takes # color looks out of the project.',
        }),
      }),
    );
  if (r.transitions.length > 0)
    out.push(
      t({
        id: 'chatPanel.proposal.transitions',
        message: plural(r.transitions.length, { one: 'Removes # transition.', other: 'Removes # transitions.' }),
      }),
    );
  if (r.effects.length > 0) out.push(t({ id: 'chatPanel.proposal.effects', message: 'Takes some effects away.' }));
  return out;
}

/**
 * The card's strip of the whole project: its clips, and a red tick for each cut, at their share of the
 * video's length.
 */
function Strip({ proposal }: { proposal: ChatProposal }) {
  const main = useProject((s) => s.snapshot?.project.timeline.main);
  const index = main ? mainIndex(main) : null;
  const end = index?.mainEndUs ?? 0;
  if (!index || end <= 0) return null;
  const at = (us: number) => `${(Math.min(Math.max(us, 0), end) / end) * 100}%`;
  return (
    <div className="p-strip" aria-hidden>
      {Array.from(index.starts, (start, i) => (
        <i
          key={`c${i}`}
          className="ov-seg"
          style={{ '--x': at(start), '--w': `${((index.ends[i] - start) / end) * 100}%` } as CSSProperties}
        />
      ))}
      {proposal.removals.ranges.map((r, i) => (
        <i key={`t${i}`} className="ov-tick" style={{ '--x': at(r.start) } as CSSProperties} />
      ))}
    </div>
  );
}

/** "Show all N cuts": each cut, tagged, with its exact range in a tooltip; hovering one lights it on the timeline. */
function CutList({ proposal }: { proposal: ChatProposal }) {
  const { t } = useLingui();
  const [open, setOpen] = useState(false);
  const tags = useMemo(() => rangeTags(proposal), [proposal]);
  const n = proposal.removals.ranges.length;
  if (n === 0) return null;
  const hover = useProposal.getState().hover;
  return (
    <>
      <button
        type="button"
        // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
        tabIndex={0}
        className="p-list-toggle"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open
          ? t({ id: 'chatPanel.proposal.hideCuts', message: 'Hide the cuts' })
          : t({ id: 'chatPanel.proposal.showCuts', message: plural(n, { one: 'Show the cut', other: 'Show all # cuts' }) })}
        <Icon name="chev-down" />
      </button>
      {open && (
        <ul className="p-list" aria-label={t({ id: 'chatPanel.proposal.cuts', message: 'Cuts' })}>
          {proposal.removals.ranges.map((r, i) => {
            const what = tags[i] || t({ id: 'chatPanel.proposal.cut', message: 'cut' });
            const from = formatPrecise(r.start);
            const exact = t({
              id: 'chatPanel.proposal.range',
              message: `${from} to ${formatPrecise(r.end)}, ${seconds(r.end - r.start)}`,
            });
            return (
              <li key={i}>
                <Tooltip label={exact} describes>
                  <button
                    type="button"
                    tabIndex={0}
                    className="p-cut"
                    aria-label={t({
                      id: 'chatPanel.proposal.cutAt',
                      message: `${what} at ${from}`,
                      comment: 'A cut in the list: its tag ("um", "pause") and where it starts',
                    })}
                    onPointerEnter={() => hover(i)}
                    onPointerLeave={() => hover(null)}
                    onFocus={() => hover(i)}
                    onBlur={() => hover(null)}
                    onClick={() => usePlayback.getState().seek(r.start)}
                  >
                    <span className="p-cut-tag">{what}</span>
                    <span className="p-cut-when">{from}</span>
                  </button>
                </Tooltip>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}

/**
 * A change that removes something waits here for the person: a raised card with a red edge, a strip of
 * the whole project with a red tick per cut, the title, the seconds it cuts in red with the video's length now and
 * after (or, for a removal that isn't footage, what goes), the list of cuts, and Apply and Discard. It never takes the
 * keyboard from the chat box (Enter there always sends): the person reaches it with Tab; Enter on the card applies and
 * Escape on the card or on Discard discards (Escape on its other buttons, such as the list of cuts, is the editor's).
 * Applied, it collapses to the applied row with its Undo chip; discarded, to "Discarded."; dropped
 * (the project changed while it was open), or replaced (a newer proposal came: one is open at a time), to a sentence
 * that says so. An Apply refused for another reason leaves it open, saying why, its Apply now Retry. An Apply Rust made
 * that the page couldn't take in closes it with a sentence that says to reopen the project. When it collapses with the
 * keyboard in it, the keyboard goes back to the chat box. Another app's proposal says which app suggested it.
 */
export function ProposalCard({ message: m }: { message: ReplyMessage }) {
  const { t } = useLingui();
  const part = m.proposal!;
  const { proposal, state } = part;
  const client = m.client;
  const focusProps = useReturnFocus(state === 'open' || state === 'applying');
  const main = useProject((s) => s.snapshot?.project.timeline.main);
  const others = useOtherRemovals(proposal);
  const cut = proposal.removedUs > 0;
  const total = seconds(proposal.removedUs);
  if (state === 'applied')
    return (
      <>
        <div key="applied" className="card applied">
          <Icon name="check" />
          <span>
            <b>{t({ id: 'chatPanel.proposal.applied', message: 'Applied.' })}</b>{' '}
            {cut
              ? t({ id: 'chatPanel.proposal.appliedCut', message: `${proposal.title}, ${total} in total.` })
              : `${proposal.title}.`}
          </span>
        </div>
        {part.undo && part.revision !== undefined && <UndoChip undo={part.undo} revision={part.revision} />}
      </>
    );
  if (state === 'discarded')
    return <p className="muted faint">{t({ id: 'chatPanel.proposal.discarded', message: 'Discarded.' })}</p>;
  if (state === 'dropped')
    return (
      <p className="muted">
        {t({
          id: 'chatPanel.proposal.dropped',
          message: 'This proposal was dropped because the project changed. Ask again to get a new one.',
        })}
      </p>
    );
  if (state === 'unshown')
    return (
      <p className="muted">
        {t({
          id: 'chatPanel.proposal.unshown',
          message: 'Applied, but the timeline didn’t update. Reopen the project.',
        })}
      </p>
    );
  if (state === 'replaced')
    return (
      <p className="muted faint">
        {t({ id: 'chatPanel.proposal.replaced', message: 'Replaced by the newer proposal below.' })}
      </p>
    );
  if (state === 'superseded')
    return (
      <p className="muted faint">
        {t({ id: 'chatPanel.proposal.superseded', message: 'Replaced by a newer proposal.' })}
      </p>
    );
  const busy = state === 'applying';
  const { apply, discard } = useChat.getState();
  const now = main ? mainIndex(main).mainEndUs : 0;
  return (
    <div
      key="card"
      role="group"
      aria-label={proposal.title}
      aria-busy={busy || undefined}
      tabIndex={0}
      {...focusProps}
      onKeyDown={(e) => {
        if (busy) return;
        // Enter on the card itself (not on one of its buttons, which press themselves).
        if (e.key === 'Enter' && e.target === e.currentTarget) {
          e.preventDefault();
          void apply(m.id);
        } else if (e.key === 'Escape' && (e.target === e.currentTarget || isDiscard(e.target))) {
          // The editor's Escape (clear the selection) doesn't also run.
          e.preventDefault();
          e.stopPropagation();
          discard(m.id);
        }
      }}
      className="card proposal"
    >
      <Strip proposal={proposal} />
      <div>
        <p className="p-title">{proposal.title}</p>
        {client !== undefined && (
          <p className="p-sub">
            {t({ id: 'chatPanel.app.suggestedBy', message: `Suggested by ${client}`, comment: '{client} is another app' })}
          </p>
        )}
        {cut && (
          <p className="p-sub">
            <b className="cut">{total}</b>{' '}
            {t({
              id: 'chatPanel.proposal.total',
              message: `in total. ${formatPrecise(now)} now, ${formatPrecise(Math.max(0, now - proposal.removedUs))} after.`,
            })}
          </p>
        )}
        {others.map((line) => (
          <p key={line} className="p-sub">
            {line}
          </p>
        ))}
      </div>
      <CutList proposal={proposal} />
      {part.reason && !busy && (
        <p className="p-why" role="alert">
          {part.reason}
        </p>
      )}
      <div className="p-actions">
        <button
          type="button"
          tabIndex={0}
          className="btn btn-primary"
          aria-disabled={busy || undefined}
          onClick={() => !busy && void apply(m.id)}
        >
          {busy
            ? t({ id: 'chatPanel.proposal.applying', message: 'Applying' })
            : part.reason
              ? t({ id: 'chatPanel.proposal.retry', message: 'Retry' })
              : t({ id: 'chatPanel.proposal.apply', message: 'Apply' })}
        </button>
        <button
          type="button"
          tabIndex={0}
          className="btn btn-secondary"
          data-discard=""
          aria-disabled={busy || undefined}
          onClick={() => !busy && discard(m.id)}
        >
          {t({ id: 'chatPanel.proposal.discard', message: 'Discard' })}
        </button>
      </div>
    </div>
  );
}
