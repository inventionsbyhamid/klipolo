import { t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { Fragment } from 'react';
import { ContextMenuItem, ContextMenuSeparator } from '../../../design/ContextMenu';
import { clipLabel, findClip, findItem, hasOwnSound, itemLabel } from '../../../lib/timeline';
import { useProject } from '../../../state/project';
import { useUi } from '../../../state/ui';
import { useCommands } from '../../command-palette/useCommands';
import { transitionLabels } from '../keys/transitionActions';
import { blockActions, clipEdits, clipSeparates, type MenuEntry } from './blockActions';

/** The menu's name for screen readers: "Actions for Recording 1". */
export function blockMenuLabel(id: string): string | undefined {
  const project = useProject.getState().snapshot?.project;
  if (!project) return undefined;
  const found = findItem(project, id);
  const clip = found ? null : findClip(project, id);
  const label = found ? itemLabel(project, found.item) : clip ? clipLabel(project, clip) : null;
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
 * runs. Mounted only while the menu is open, so only an open menu follows the selection and the playhead. A clip's
 * edits and Separate items are its card's too (`blockActions`).
 */
export function BlockMenuItems({ blockId, kind }: { blockId: string; kind: 'clip' | 'layer' }) {
  const { t } = useLingui();
  const commands = useCommands();
  const project = useProject((s) => s.snapshot?.project);
  const expanded = useUi((s) => s.timelineExpanded);
  const found = project ? findItem(project, blockId) : null;
  // A clip with one after it and no transition into it yet (one it has is selected at its cut). A video's only clip
  // lists it off, saying why: a transition goes between two clips.
  const main = project?.timeline.main ?? [];
  const at = found ? -1 : main.findIndex((clip) => clip.id === blockId);
  const transitionAfter = at >= 0 && at + 1 < main.length && !main[at].transitionOut;
  const onlyClip = at === 0 && main.length === 1;

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
  const groups: MenuEntry[][] =
    kind === 'clip'
      ? [
          [
            split,
            ...(project && at >= 0 ? clipEdits(project, at) : []),
            ...(transitionAfter || onlyClip ? [addTransition] : []),
          ],
          [{ id: 'copy' }, { id: 'cut' }, { id: 'duplicate' }, { id: 'paste-effects' }],
          sounds,
          project && at >= 0 ? clipSeparates(project, at, commands) : [],
          [{ id: 'delete' }],
        ]
      : [
          [split, { id: 'attach' }, ...(mutes ? [{ id: 'sound-mute' }] : [])],
          // Keys set by hand: listed while the layer has some.
          ...(found?.item.layout.motion?.keys ? [[{ id: 'motion-keys-clear' }]] : []),
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

  return blockActions(blockId, groups, commands).map((group, i) => (
    <Fragment key={i}>
      {i > 0 && <ContextMenuSeparator />}
      {group.map((action) => (
        <ContextMenuItem
          key={action.id}
          shortcut={action.shortcut}
          disabled={action.disabled}
          note={action.note}
          // On this block alone: Split at playhead on a clip splits that clip, or says why it can't.
          onSelect={action.run}
        >
          {action.label}
        </ContextMenuItem>
      ))}
    </Fragment>
  ));
}
