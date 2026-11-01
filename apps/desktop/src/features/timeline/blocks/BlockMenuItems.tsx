import { t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { Fragment } from 'react';
import { ContextMenuItem, ContextMenuSeparator } from '../../../design/ContextMenu';
import { recordingCameras } from '../../../lib/camera';
import { findClip, findItem, hasOwnSound, itemLabel, sourceLabel } from '../../../lib/timeline';
import { useProject } from '../../../state/project';
import { useSelection } from '../../../state/selection';
import { useUi } from '../../../state/ui';
import type { Command } from '../../command-palette/command';
import { useCommands } from '../../command-palette/useCommands';
import { transitionLabels } from '../keys/transitionActions';

/** The menu's name for screen readers: "Actions for Recording 1". */
export function blockMenuLabel(id: string): string | undefined {
  const project = useProject.getState().snapshot?.project;
  if (!project) return undefined;
  const found = findItem(project, id);
  const clip = found ? null : findClip(project, id);
  const label = found ? itemLabel(project, found.item) : clip ? sourceLabel(project, clip.source) : null;
  if (label === null) return undefined;
  return t({
    id: 'timeline.menu.name',
    message: `Actions for ${label}`,
    comment: 'A timeline block’s right-click menu, for screen readers; {label} is a clip’s or layer’s name',
  });
}

/**
 * What block `blockId`'s menu lists, in groups. Each item runs the same command as its shortcut, so the menu,
 * the keys and the player bar always agree; the block was selected when the menu opened, and is again before an item
 * runs. Mounted only while the menu is open, so only an open menu follows the selection and the playhead.
 */
export function BlockMenuItems({ blockId, kind }: { blockId: string; kind: 'clip' | 'layer' }) {
  const { t } = useLingui();
  const commands = useCommands();
  const project = useProject((s) => s.snapshot?.project);
  const expanded = useUi((s) => s.timelineExpanded);
  const byId = new Map(commands.map((c) => [c.id, c]));
  const found = project ? findItem(project, blockId) : null;
  // A clip with one after it and no transition into it yet (one it has is selected at its cut). A video's only clip
  // lists it off, saying why: a transition goes between two clips.
  const main = project?.timeline.main ?? [];
  const at = found ? -1 : main.findIndex((clip) => clip.id === blockId);
  const transitionAfter = at >= 0 && at + 1 < main.length && !main[at].transitionOut;
  const onlyClip = at === 0 && main.length === 1;
  // A recording clip with a camera beside its footage: Separate camera (off, saying why, once it is a layer).
  const hasCamera = at >= 0 && !!project && !!main[at].camera && recordingCameras(project, main[at]).length > 0;

  // A sound layer, or a video layer whose file has sound, has Mute.
  const mutes = !!project && !!found && hasOwnSound(project, found.item);

  const split = { id: 'split', label: t({ id: 'timeline.menu.split', message: 'Split at playhead' }) };
  // The layer order (⌥↑ / ⌥↓): a picture layer is brought forward or sent back, over or under the others; a sound or
  // zoom layer moves to the track above or below.
  const picture = found?.track.kind === 'overlay';
  const order = [
    { id: 'track-up', label: picture ? t({ id: 'timeline.menu.forward', message: 'Bring forward' }) : undefined },
    { id: 'track-down', label: picture ? t({ id: 'timeline.menu.back', message: 'Send back' }) : undefined },
    // While every track shows, a layer can go onto a track of its own.
    ...(expanded ? [{ id: 'layer-new-track' }] : []),
  ];
  const addTransition = {
    id: 'add-transition',
    label: t({
      id: 'timeline.menu.addTransition',
      message: 'Add transition after',
      comment: 'A clip’s menu item: a transition from this clip into the next one',
    }),
    off: onlyClip ? transitionLabels.needsTwo() : undefined,
  };
  // A recording clip's sounds (`useSourceCommands`): their mutes in this clip, then Separate beside the camera's.
  const sounds = commands.filter((c) => c.id.startsWith('source-mute:')).map((c) => ({ id: c.id }));
  const separates = commands.filter((c) => c.id.startsWith('source-separate:')).map((c) => ({ id: c.id }));
  // `off`: listed, but off here, with why.
  const groups: { id: string; label?: string; off?: string }[][] =
    kind === 'clip'
      ? [
          [split, ...(transitionAfter || onlyClip ? [addTransition] : [])],
          [{ id: 'copy' }, { id: 'cut' }, { id: 'duplicate' }, { id: 'paste-effects' }],
          sounds,
          [...(hasCamera ? [{ id: 'separate-camera' }] : []), ...separates],
          [{ id: 'delete' }],
        ]
      : [
          [split, { id: 'attach' }, ...(mutes ? [{ id: 'sound-mute' }] : [])],
          order,
          [
            { id: 'copy' },
            { id: 'cut' },
            { id: 'duplicate' },
            // Sound and zoom layers have no effects.
            ...(found?.track.kind === 'overlay' ? [{ id: 'paste-effects' }] : []),
          ],
          [{ id: 'delete' }],
        ];

  const item = ({ id, label, off }: { id: string; label?: string; off?: string }, command: Command) => (
    <ContextMenuItem
      key={id}
      shortcut={typeof command.shortcut === 'string' ? command.shortcut : undefined}
      disabled={off !== undefined || !command.enabled}
      note={off ?? (command.enabled ? undefined : command.reason)}
      onSelect={() => {
        useSelection.getState().select(blockId);
        // On this block alone: Split at playhead on a clip splits that clip, or says why it can't.
        command.run({ block: blockId });
      }}
    >
      {label ?? command.label}
    </ContextMenuItem>
  );
  return groups
    .map((group) => group.flatMap((entry) => (byId.has(entry.id) ? [item(entry, byId.get(entry.id)!)] : [])))
    .filter((group) => group.length > 0)
    .map((group, i) => (
      <Fragment key={i}>
        {i > 0 && <ContextMenuSeparator />}
        {group}
      </Fragment>
    ));
}
