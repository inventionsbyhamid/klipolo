import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Clip } from '../../bindings/Clip';
import type { Project } from '../../bindings/Project';
import { recordingCameras } from '../../lib/camera';
import { labelText } from '../../lib/labels';
import { findItem, hasOwnSound, itemEndUs } from '../../lib/timeline';
import { useClipboard } from '../../state/clipboard';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { searchWords, type Command } from '../command-palette/command';
import { copy, cut, duplicate, paste, pasteEffects } from './keys/clipboardActions';
import { trackStepOps } from './keys/keyOps';
import { nudge } from './keys/nudge';
import { muteLabel } from '../preview';
import { separateCamera, stepTrack, toggleAttach, toggleSoundMute } from './keys/layerActions';
import { split, trimToPlayhead } from './keys/timelineActions';
import { clipIndexAt, mainIndex, type MainIndex } from './model/index';
import { useTimelineNavCommands } from './useTimelineNavCommands';
import { useTransitionCommands } from './useTransitionCommands';

const NO_CLIPS: Clip[] = [];

/**
 * Main clip `i`'s camera, for Separate camera (what `detachSource` takes): none (no recording camera), separable, or
 * separated (one of its files already plays in a layer over the clip).
 */
function cameraOf(project: Project, index: MainIndex, i: number): 'none' | 'separable' | 'separated' {
  const clip = project.timeline.main[i];
  const cameras = recordingCameras(project, clip);
  if (!clip.camera || cameras.length === 0) return 'none';
  const shown = project.timeline.tracks.some((track) =>
    track.items.some(
      (item) =>
        item.content.type === 'video' &&
        cameras.includes(item.content.assetId) &&
        item.startUs < index.ends[i] &&
        itemEndUs(item) > index.starts[i],
    ),
  );
  return shown ? 'separated' : 'separable';
}

/**
 * The timeline's commands: split, trims to the playhead, nudges, the clipboard, track moves, attaching,
 * a sound or video layer's mute and Separate camera, plus `useTransitionCommands`' (Add transition, ⌘T) and
 * `useTimelineNavCommands`' (selection steps, edit points, zoom, snapping). Each has a stable id, is `enabled` only
 * when it can act on the selection, and runs the same code from its shortcut, the player bar and the block menu.
 * `useCommands` merges them.
 */
export function useTimelineCommands(): Command[] {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  const clipboard = useClipboard();
  const nav = useTimelineNavCommands();
  const transitions = useTransitionCommands();
  const index = mainIndex(project?.timeline.main ?? NO_CLIPS);
  // The playhead changes every frame: this re-renders only when it flips (a run reads the playhead then).
  const clipUnderPlayhead = usePlayback((s) => clipIndexAt(index, s.playheadUs) >= 0);
  return useMemo(() => {
    if (!project) return [];
    const main = project.timeline.main;
    const found = selectedId ? findItem(project, selectedId) : null;
    const clipAt = selectedId && !found ? main.findIndex((c) => c.id === selectedId) : -1;
    const id = selectedId ?? '';
    const clip = clipAt >= 0;
    const block = clip || found !== null;
    // A layer on a locked track can be copied, nothing more.
    const layer = found && !found.track.locked ? found : null;
    // A sound layer, or a video layer whose file has sound: its own switch mutes it. Named as it stands even on a
    // locked track, where it is off.
    const content = found?.item.content;
    const mutable =
      found && hasOwnSound(project, found.item) && (content?.type === 'audio' || content?.type === 'video')
        ? content
        : null;
    const ours = clipboard.projectId === project.id;
    // The commands' names are their undo labels, in the language shown.
    const label = {
      clip: labelText('undo.split.clip'),
      layer: labelText('undo.split.layer'),
      up: labelText('undo.trackUp'),
      down: labelText('undo.trackDown'),
      attach: labelText(found?.item.attach ? 'undo.attach.free' : 'undo.attach.keep'),
      mute: labelText(muteLabel(!mutable?.muted, mutable?.type === 'video' ? 'video' : 'sound')),
      camera: labelText('undo.separateCamera'),
    };
    const camera = clip ? cameraOf(project, index, clipAt) : 'none';

    const edits: Command[] = [
      {
        id: 'split',
        action: found ? 'splitItem' : 'split',
        group: 'edit',
        label: found ? label.layer : label.clip,
        shortcut: 'S',
        keywords: searchWords(
          t({
            id: 'command.split.keywords',
            message: 'cut',
            comment: 'Comma-separated search words for Split',
          }),
        ),
        // With a layer selected, S acts on that layer alone: never on the clip under it, which the user didn't pick.
        enabled: found !== null || clipUnderPlayhead,
        run: (options) => split(selectedId, options?.block),
      },
      {
        id: 'trim-start',
        action: clip ? 'trimClip' : 'trimItem',
        group: 'edit',
        label: t({ id: 'command.trimStart', message: 'Trim start to playhead' }),
        shortcut: 'Q',
        // Whenever a block is selected: it says why it can't act (as S does).
        enabled: block,
        run: (options) => trimToPlayhead(id, 'start', options?.repeat),
      },
      {
        id: 'trim-end',
        action: clip ? 'trimClip' : 'trimItem',
        group: 'edit',
        label: t({ id: 'command.trimEnd', message: 'Trim end to playhead' }),
        shortcut: 'W',
        enabled: block,
        run: (options) => trimToPlayhead(id, 'end', options?.repeat),
      },
      {
        id: 'nudge-back',
        action: clip ? 'moveClip' : 'moveItem',
        group: 'edit',
        label: t({
          id: 'command.nudgeBack',
          message: 'Move earlier',
          comment: 'A layer a frame earlier, or a clip a place',
        }),
        shortcut: ',',
        // Whenever a block is selected: it says why it can't move (first, at the start, locked).
        enabled: block,
        // ⇧: ten frames (a clip still moves one place).
        run: (options) => nudge(id, clip || !options?.large ? -1 : -10, options?.repeat),
      },
      {
        id: 'nudge-forward',
        action: clip ? 'moveClip' : 'moveItem',
        group: 'edit',
        label: t({
          id: 'command.nudgeForward',
          message: 'Move later',
          comment: 'A layer a frame later, or a clip a place',
        }),
        shortcut: '.',
        enabled: block,
        run: (options) => nudge(id, clip || !options?.large ? 1 : 10, options?.repeat),
      },
      {
        id: 'copy',
        action: 'clipboard',
        group: 'edit',
        label: t({ id: 'timeline.copy', message: 'Copy', comment: 'Command' }),
        shortcut: '⌘C',
        enabled: block,
        run: () => copy(id),
      },
      {
        id: 'cut',
        action: 'clipboard',
        group: 'edit',
        label: labelText('undo.cut'),
        shortcut: '⌘X',
        enabled: clip || layer !== null,
        run: () => cut(id),
      },
      {
        id: 'paste',
        action: 'clipboard',
        group: 'edit',
        label: labelText('undo.paste'),
        shortcut: '⌘V',
        enabled: ours && clipboard.entry !== null,
        run: paste,
      },
      {
        id: 'duplicate',
        action: clip ? 'insertClip' : 'addItem',
        group: 'edit',
        label: labelText('undo.duplicate'),
        shortcut: '⌘D',
        enabled: clip || layer !== null,
        run: () => duplicate(id),
      },
      {
        id: 'paste-effects',
        action: 'clipboard',
        group: 'edit',
        label: labelText('undo.pasteEffects'),
        shortcut: '⌥⌘V',
        // Sound and zoom layers have no effects.
        enabled: ours && clipboard.effects !== null && (clip || layer?.track.kind === 'overlay'),
        run: () => pasteEffects(id),
      },
      {
        id: 'track-up',
        action: 'moveItem',
        group: 'edit',
        label: label.up,
        shortcut: '⌥↑',
        enabled: layer !== null && trackStepOps(project, id, 1).length > 0,
        run: () => stepTrack(id, 1),
      },
      {
        id: 'track-down',
        action: 'moveItem',
        group: 'edit',
        label: label.down,
        shortcut: '⌥↓',
        enabled: layer !== null && trackStepOps(project, id, -1).length > 0,
        run: () => stepTrack(id, -1),
      },
      {
        id: 'attach',
        action: 'setAttach',
        group: 'edit',
        label: label.attach,
        // Only a layer over the main track can be attached (past its end, only layers play).
        enabled: layer !== null && (layer.item.attach !== null || layer.item.startUs < index.mainEndUs),
        run: () => toggleAttach(id),
      },
      {
        id: 'sound-mute',
        action: 'updateItem',
        group: 'edit',
        label: label.mute,
        keywords: searchWords(
          t({
            id: 'command.soundMute.keywords',
            message: 'audio, silence, sound off, sound on',
            comment: 'Comma-separated search words for Mute sound and Unmute sound',
          }),
        ),
        // A sound or video layer's own switch, as the inspector's Mute: its volume stays for when it plays again.
        enabled: layer !== null && mutable !== null,
        run: () => toggleSoundMute(id),
      },
      {
        id: 'separate-camera',
        action: 'detachSource',
        group: 'edit',
        label: label.camera,
        // A recording clip's camera, once: a camera already in a layer of its own is off, saying so.
        enabled: camera === 'separable',
        reason:
          camera === 'separated'
            ? t({ id: 'command.separateCamera.done', message: "This clip's camera is already in its own layer." })
            : undefined,
        run: () => separateCamera(id),
      },
    ];
    return [...edits, ...transitions, ...nav];
  }, [t, project, selectedId, clipboard, nav, transitions, index, clipUnderPlayhead]);
}
