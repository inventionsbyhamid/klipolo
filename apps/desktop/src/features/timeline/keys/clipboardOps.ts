import type { Clip } from '../../../bindings/Clip';
import type { EffectInstance } from '../../../bindings/EffectInstance';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { newId } from '../../../lib/defaults';
import { snapToFrame } from '../../../lib/time';
import { findClip, findItem } from '../../../lib/timeline';
import { useClipboard, type ClipboardEntry } from '../../../state/clipboard';
import { clipIndexAt, mainIndex } from '../model/index';

/*
 * Copy, paste, duplicate and paste effects. The clipboard holds deep copies of project data; pasting and
 * duplicating compile them to `addItem` / `insertClip` ops with new ids, which chat's tools can send too.
 */

/** A copy of a clip with a new id and no transition (it comes in as a cut: its old neighbour isn't there). */
function clipCopy(clip: Clip): Clip {
  const copy: Clip = { ...structuredClone(clip), id: newId() };
  delete copy.transitionOut;
  return copy;
}

/** ⌘C: a copy of clip or layer `id`, and its effect stack, go on the clipboard. The entry, or null if there is none. */
export function copySelection(project: Project, id: string): ClipboardEntry | null {
  const found = findItem(project, id);
  const clip = found ? null : findClip(project, id);
  const entry: ClipboardEntry | null = found
    ? { kind: 'item', item: structuredClone(found.item) }
    : clip
      ? { kind: 'clip', clip: structuredClone(clip) }
      : null;
  if (!entry) return null;
  const stack = entry.kind === 'item' ? entry.item.effects : entry.clip.effects;
  useClipboard.setState({
    projectId: project.id,
    entry,
    group: null,
    effects: stack.length > 0 ? structuredClone(stack) : null,
  });
  return entry;
}

/**
 * ⌘V: a layer at the playhead, on the first free track of its kind, attached to the clip there when it's a picture
 * layer inside the video; a clip after the clip under the playhead (at the end past it).
 */
export function pasteOps(project: Project, entry: ClipboardEntry, playheadUs: number): Op[] {
  const at = snapToFrame(playheadUs, project.settings.fps);
  const index = mainIndex(project.timeline.main);
  if (entry.kind === 'item') {
    const item = { ...structuredClone(entry.item), id: newId(), startUs: at, attach: null };
    const attach = item.content.type !== 'audio' && at < index.mainEndUs;
    return [{ type: 'addItem', track: { type: 'auto' }, item, attach }];
  }
  const under = clipIndexAt(index, at);
  return [{ type: 'insertClip', clip: clipCopy(entry.clip), index: under >= 0 ? under + 1 : null }];
}

/** ⌘D: a layer right after itself on its own track (attached like it), or a clip right after itself. */
export function duplicateOps(project: Project, id: string): Op[] {
  const found = findItem(project, id);
  if (found) {
    const { item, track } = found;
    if (track.locked) return [];
    const copy = {
      ...structuredClone(item),
      id: newId(),
      startUs: item.startUs + item.durationUs,
      attach: null,
    };
    return [
      { type: 'addItem', track: { type: 'track', id: track.id }, item: copy, attach: item.attach !== null },
    ];
  }
  const i = project.timeline.main.findIndex((clip) => clip.id === id);
  return i < 0 ? [] : [{ type: 'insertClip', clip: clipCopy(project.timeline.main[i]), index: i + 1 }];
}

/**
 * ⌥⌘V: `effects` (ids kept: they only need to be unique within a stack) replace the stack of clip `id`, or of layer
 * `id` when it draws (sound and zoom layers have no effects) on an unlocked track.
 */
export function pasteEffectsOps(project: Project, id: string, effects: EffectInstance[]): Op[] {
  const found = findItem(project, id);
  if (found) {
    if (found.track.locked || found.track.kind !== 'overlay') return [];
    return [{ type: 'updateItem', id, patch: { effects: structuredClone(effects) } }];
  }
  return findClip(project, id)
    ? [{ type: 'updateClip', id, patch: { effects: structuredClone(effects) } }]
    : [];
}
