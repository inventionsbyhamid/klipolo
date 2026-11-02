import type { Project } from '../../../bindings/Project';
import { recordingCameras } from '../../../lib/camera';
import { clipReverseState, clipSoundState, isFreeze } from '../../../lib/timeline';
import { useProject } from '../../../state/project';
import { useSelection } from '../../../state/selection';
import type { Command } from '../../command-palette/command';
import { useCommands } from '../../command-palette/useCommands';

/** A menu entry: a command by id, with a label of its own when it needs one; `off`: listed, but off here, with why. */
export interface MenuEntry {
  id: string;
  label?: string;
  off?: string;
}

/** A block menu's item as a menu shows it, from its command. */
export interface BlockAction {
  id: string;
  label: string;
  /** Its keyboard shortcut ("⌥F"), when it has one key combination. */
  shortcut?: string;
  disabled: boolean;
  /** Why it is off. */
  note?: string;
  /** Runs it on its block alone, selected first. */
  run: () => void;
}

/**
 * Main clip `at`'s own edits as its menus list them: Freeze frame, and Reverse for an imported video (off, saying why,
 * while its sound is in a layer).
 */
export function clipEdits(project: Project, at: number): MenuEntry[] {
  const turns = ['backward', 'forward', 'separated'].includes(clipReverseState(project, at));
  return [{ id: 'freeze-frame' }, ...(turns ? [{ id: 'reverse' }] : [])];
}

/**
 * Main clip `at`'s Separate items as its menus list them: Separate camera for a recording clip with a camera beside its
 * footage (off, saying why, once it is a layer), Separate sound for an imported clip whose file has sound (off, saying
 * why, once it is a layer or while sped up), and each recorded sound's Separate (`useSourceCommands`, the selected
 * clip's).
 */
export function clipSeparates(project: Project, at: number, commands: Command[]): MenuEntry[] {
  const clip = project.timeline.main[at];
  const hasCamera = !!clip.camera && !isFreeze(clip) && recordingCameras(project, clip).length > 0;
  const hasSound = clipSoundState(project, at) !== 'none';
  return [
    ...(hasCamera ? [{ id: 'separate-camera' }] : []),
    ...(hasSound ? [{ id: 'separate-sound' }] : []),
    ...commands.filter((c) => c.id.startsWith('source-separate:')).map((c) => ({ id: c.id })),
  ];
}

/**
 * `groups` as block `blockId`'s menu items: each runs the same command as its shortcut, on that block alone (selected
 * again first), so every menu and the keys agree. An entry with no command is left out, and so is a group left empty.
 */
export function blockActions(blockId: string, groups: MenuEntry[][], commands: Command[]): BlockAction[][] {
  const byId = new Map(commands.map((c) => [c.id, c]));
  return groups
    .map((group) =>
      group.flatMap(({ id, label, off }): BlockAction[] => {
        const command = byId.get(id);
        if (!command) return [];
        return [
          {
            id,
            label: label ?? command.label,
            shortcut: typeof command.shortcut === 'string' ? command.shortcut : undefined,
            disabled: off !== undefined || !command.enabled,
            note: off ?? (command.enabled ? undefined : command.reason),
            run: () => {
              useSelection.getState().select(blockId);
              command.run({ block: blockId });
            },
          },
        ];
      }),
    )
    .filter((group) => group.length > 0);
}

/**
 * Main clip `clipId`'s own actions as its card's more menu lists them, as its block menu does: its edits, then its
 * Separate items. Mount it only while the menu is open: the commands follow the selection and the playhead.
 */
export function useClipActions(clipId: string): BlockAction[][] {
  const commands = useCommands();
  const project = useProject((s) => s.snapshot?.project);
  const at = project ? project.timeline.main.findIndex((c) => c.id === clipId) : -1;
  if (!project || at < 0) return [];
  return blockActions(clipId, [clipEdits(project, at), clipSeparates(project, at, commands)], commands);
}
