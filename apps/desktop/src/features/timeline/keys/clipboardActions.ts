import { t } from '@lingui/core/macro';
import { formatTime } from '../../../lib/time';
import { itemLabel, sourceLabel } from '../../../lib/timeline';
import { useClipboard } from '../../../state/clipboard';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { announce } from '../announce';
import { copySelection, duplicateOps, pasteEffectsOps, pasteOps } from './clipboardOps';
import { deleteOps } from './keyOps';
import { blockLabel, commit, editable, selectNew } from './timelineActions';

/** ⌘C: block `id` onto the clipboard (nothing is edited). */
export function copy(id: string) {
  const project = useProject.getState().snapshot?.project;
  if (!project || !copySelection(project, id)) return;
  const label = blockLabel(project, id);
  announce(t({ id: 'timeline.key.copied', message: `Copied ${label}` }));
}

/** ⌘X: copy, then the delete command's ops, as one undo step called Cut. */
export function cut(id: string) {
  keyBurst.after(() => {
    const project = editable();
    const ops = project ? deleteOps(project, id) : [];
    if (!project || ops.length === 0 || !copySelection(project, id)) return;
    const label = blockLabel(project, id);
    commit(
      'undo.cut',
      ops,
      t({
        id: 'timeline.key.cut',
        message: `Cut ${label}`,
        comment: 'Screen-reader note: the block was cut',
      }),
    );
  });
}

/** ⌘V: the clipboard's block at the playhead (a clip after the clip there), then selected. */
export function paste() {
  keyBurst.after(() => {
    const project = editable();
    const { entry, projectId } = useClipboard.getState();
    if (!project || !entry || projectId !== project.id) return;
    const at = usePlayback.getState().playheadUs;
    const ops = pasteOps(project, entry, at);
    const op = ops[0];
    const id = op.type === 'addItem' ? op.item.id : op.type === 'insertClip' ? op.clip.id : null;
    const label =
      entry.kind === 'item' ? itemLabel(project, entry.item) : sourceLabel(project, entry.clip.source);
    const time = formatTime(at);
    commit('undo.paste', ops, t({ id: 'timeline.key.pasted', message: `Pasted ${label} at ${time}` }), () =>
      selectNew(id),
    );
  });
}

/** ⌘D: a copy of block `id` right after it, then selected. */
export function duplicate(id: string) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const ops = duplicateOps(project, id);
    const op = ops[0];
    const copyId = op?.type === 'addItem' ? op.item.id : op?.type === 'insertClip' ? op.clip.id : null;
    const label = blockLabel(project, id);
    commit('undo.duplicate', ops, t({ id: 'timeline.key.duplicated', message: `Duplicated ${label}` }), () =>
      selectNew(copyId),
    );
  });
}

/** ⌥⌘V: the copied block's effects onto block `id`. */
export function pasteEffects(id: string) {
  keyBurst.after(() => {
    const project = editable();
    const { effects, projectId } = useClipboard.getState();
    if (!project || !effects || projectId !== project.id) return;
    const label = blockLabel(project, id);
    commit(
      'undo.pasteEffects',
      pasteEffectsOps(project, id, effects),
      t({ id: 'timeline.key.effectsPasted', message: `Pasted effects onto ${label}` }),
    );
  });
}
