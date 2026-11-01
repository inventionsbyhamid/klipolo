import { useLingui } from '@lingui/react/macro';
import { Eye, EyeOff, Headphones, Lock, LockOpen, Volume2, VolumeX, type LucideIcon } from 'lucide-react';
import { useEffect, useId, useRef, useState } from 'react';
import type { Track } from '../../bindings/Track';
import { cn } from '../../design/cn';
import { IconButton } from '../../design/IconButton';
import { useLocale } from '../../i18n/react';
import { renameTrack, setTrackFlag, startRename, useTrackRename } from './keys/trackActions';
import { trackStateName, trackToggleName, type TrackState } from './keys/trackLabels';
import { TrackMenu } from './lanes/TrackMenu';
import { TRACK_NAME_MAX, trackHas, trackToggles, type TrackToggle } from './model/trackOps';

function toggleIcon(track: Track, field: TrackToggle): LucideIcon {
  if (field === 'hidden') return track.hidden ? EyeOff : Eye;
  if (field === 'muted') return track.muted ? VolumeX : Volume2;
  if (field === 'solo') return Headphones;
  return track.locked ? Lock : LockOpen;
}

/**
 * A toggle, while the header shows them: 20 px, icon only, so all three fit in the column between the name (which keeps
 * 33 px meanwhile) and the options button. They never reach over the lane.
 */
export const TRACK_TOGGLE_PX = 20;
const TOGGLE_SIZE = { width: TRACK_TOGGLE_PX, height: TRACK_TOGGLE_PX };
/** The gap before the toggles, while the header shows them (after the name). */
export const TRACK_TOGGLES_GAP_PX = 2;
const TOGGLES_GAP = { marginLeft: TRACK_TOGGLES_GAP_PX };
/** The header's padding (6 px left, 2 px right) and its 1 px right border. */
export const TRACK_HEADER_CHROME_PX = 9;
/** The badge that shows a track's states at rest (hidden or muted, soloed, locked): 12 px wide and a 2 px gap. */
export const TRACK_STATE_PX = 14;

/**
 * What `track` is at rest, if anything: hidden or muted, soloed, locked, or some of those (the badge's icons, stacked,
 * and its name).
 */
function restState(track: Track): { icons: LucideIcon[]; state: TrackState } | null {
  const audio = track.kind === 'audio';
  const off = audio ? track.muted : track.hidden;
  const soloed = !!track.solo && trackHas(track, 'solo');
  const icons = [
    ...(off ? [audio ? VolumeX : EyeOff] : []),
    ...(soloed ? [Headphones] : []),
    ...(track.locked ? [Lock] : []),
  ];
  if (icons.length === 0) return null;
  const words = [off ? (audio ? 'muted' : 'hidden') : '', soloed ? 'soloed' : '', track.locked ? 'locked' : ''];
  // "hidden" + "Soloed" + "Locked": hiddenSoloedLocked, one of `TrackState`'s names.
  const state = words
    .filter(Boolean)
    .map((w, i) => (i === 0 ? w : w[0].toUpperCase() + w.slice(1)))
    .join('') as TrackState;
  return { icons, state };
}

/**
 * A track's header while every track shows: the name (double-click it, or the options' Rename, to edit it), its
 * toggles (hide or mute, solo on overlay and audio tracks, lock) and the track's options. At rest the name gets the
 * header's width less the options button, and a 12 px badge shows what the track is (hidden or muted, soloed, locked,
 * its icons stacked). While the pointer is over the header or focus is in it, the toggles show in its own row, between
 * the name and the options, never over the lane: a press at the lane's start always reaches the layer there. Every
 * toggle is in the track's options too. A toggle is always the same element, so it keeps focus as it flips. Each
 * control runs a track action (`trackActions`), one undo step.
 */
export function TrackHeader({ track }: { track: Track }) {
  // The toggles' names come from `trackToggleName` (the core `t`): a language switch re-renders the header.
  useLocale();
  const renaming = useTrackRename((s) => s.id === track.id);
  const nameId = useId();
  const stateId = useId();
  const menuRef = useRef<HTMLButtonElement>(null);
  // A rename ended from the keyboard (Enter, Escape) puts focus back on the track's options button, not the page.
  const refocus = useRef(false);
  useEffect(() => {
    if (renaming || !refocus.current) return;
    refocus.current = false;
    menuRef.current?.focus({ preventScroll: true });
  }, [renaming]);
  const toggles = trackToggles(track);
  const rest = restState(track);
  return (
    <div data-track-header className="tl-track-head group/track flex h-full items-center pr-0.5 pl-1.5">
      {renaming ? (
        <TrackNameField track={track} onDone={(keyboard) => (refocus.current = keyboard)} />
      ) : (
        <span
          id={nameId}
          className="min-w-0 flex-1 truncate text-[12px] text-fg-muted"
          title={track.name}
          onDoubleClick={() => startRename(track.id)}
        >
          {track.name}
        </span>
      )}
      {rest && (
        <span
          id={stateId}
          role="img"
          aria-label={trackStateName(rest.state)}
          data-track-state
          className={cn(
            'mr-0.5 flex w-3 shrink-0 flex-col items-center justify-center gap-0.5 text-fg-muted',
            'group-focus-within/track:hidden group-hover/track:hidden',
          )}
        >
          {rest.icons.map((Glyph, i) => (
            // Three states stack in a short row at 10 px.
            <Glyph key={i} size={rest.icons.length > 2 ? 8 : 10} aria-hidden />
          ))}
        </span>
      )}
      {/* At rest the toggles wait out of the way (no room taken, nor a Tab stop). Pointed at or focused, they show in
          the header's row at 20 px, the name giving them room, so nothing reaches over the lane. Not while the name is
          being edited: the field keeps the row. */}
      <span
        data-at-rest="hidden"
        data-track-toggles
        data-renaming={renaming || undefined}
        style={TOGGLES_GAP}
        className={cn(
          'hidden shrink-0 items-center',
          !renaming && 'group-focus-within/track:flex group-hover/track:flex',
        )}
      >
        {toggles.map((field) => {
          const Glyph = toggleIcon(track, field);
          return (
            <IconButton
              key={field}
              icon={<Glyph size={13} strokeWidth={1.8} aria-hidden />}
              label={trackToggleName(track, field)}
              style={TOGGLE_SIZE}
              // Solo's icon stays the same: it is drawn on while the track is soloed (its name says what it does).
              className={cn('tl-track-toggle', field === 'solo' && track.solo && 'is-on')}
              // The other state from what the track has when the press goes in.
              onClick={() => setTrackFlag(track.id, field)}
            />
          );
        })}
      </span>
      <TrackMenu track={track} describedBy={rest ? `${nameId} ${stateId}` : nameId} ref={menuRef} />
    </div>
  );
}

/**
 * The name as a field: Enter or leaving it renames the track (a trimmed, changed, non-empty name), Escape keeps the old
 * one. `onDone(keyboard)` runs once, as it closes.
 */
function TrackNameField({ track, onDone }: { track: Track; onDone: (keyboard: boolean) => void }) {
  const { t } = useLingui();
  const done = useRef(false);
  const [value, setValue] = useState(track.name);
  const finish = (keep: boolean, keyboard: boolean) => {
    if (done.current) return;
    done.current = true;
    onDone(keyboard);
    useTrackRename.setState({ id: null });
    if (keep) renameTrack(track.id, value);
  };
  return (
    <input
      aria-label={t({ id: 'timeline.track.nameField', message: 'Track name' })}
      value={value}
      onChange={(e) => setValue(e.target.value)}
      maxLength={TRACK_NAME_MAX}
      // The field takes focus as it opens (the options' Rename, a double-click), its text selected.
      autoFocus
      onFocus={(e) => e.currentTarget.select()}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          finish(true, true);
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          finish(false, true);
        }
      }}
      onBlur={() => finish(true, false)}
      className="tl-track-name-field"
    />
  );
}
