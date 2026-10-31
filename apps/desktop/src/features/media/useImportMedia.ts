import { plural, t } from '@lingui/core/macro';
import { useCallback } from 'react';
import type { Asset } from '../../bindings/Asset';
import type { SkippedFile } from '../../bindings/SkippedFile';
import { bridge } from '../../bridge/commands';
import { errorMessage } from '../../lib/errors';
import { placementOps } from '../../lib/placement';
import { flushBursts } from '../../state/bursts';
import { afterCommits, beginCommit, keepAtClose } from '../../state/commitQueue';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { useUi, type LibraryTab } from '../../state/ui';
import { dropAllOps, type DropTarget } from '../timeline/drops';
import { isMusicTarget } from './addMedia';
import { addMusic } from './place';

/** A sound at least this long is music in the Library; shorter ones are sound effects. */
export const MUSIC_MIN_US = 15_000_000;

/** The Library tab a file shows under. */
export function tabOf(asset: Asset): LibraryTab {
  if (asset.kind === 'image') return 'images';
  if (asset.kind === 'audio') return asset.durationUs >= MUSIC_MIN_US ? 'music' : 'sounds';
  return 'clips';
}

const NAMES_SHOWN = 3;

function listNames(names: string[]): string {
  const shown = names.slice(0, NAMES_SHOWN).join(
    t({
      id: 'media.skipped.separator',
      message: ', ',
      comment: 'Goes between file names in a list (“a.heic, b.pdf”)',
    }),
  );
  if (names.length <= NAMES_SHOWN) return shown;
  const more = names.length - NAMES_SHOWN;
  // The names stay outside the plural: a `#` in one ("Take #3.mov") is part of the name, not the count.
  const andMore = t({
    id: 'media.skipped.namesMore',
    message: plural(more, { one: 'and # more', other: 'and # more' }),
    comment: 'After the first file names, e.g. “a.heic, b.pdf, c.zip and 2 more”',
  });
  return `${shown} ${andMore}`;
}

/**
 * One line naming every skipped file with Rust's reason, e.g. "Skipped 2 files. a.heic, b.pdf: this file type isn't
 * supported." Each reason reads as the rest of its sentence (first letter lowercased).
 */
function skippedMessage(skipped: SkippedFile[]): string {
  const byReason = new Map<string, string[]>();
  for (const file of skipped) byReason.set(file.reason, [...(byReason.get(file.reason) ?? []), file.name]);
  const parts = [...byReason]
    .map(([reason, names]) => {
      const files = listNames(names);
      const why = `${reason.charAt(0).toLowerCase()}${reason.slice(1)}`;
      return t({
        id: 'media.skipped.part',
        message: `${files}: ${why}`,
        comment: '{files} lists file names; {why} is why they were skipped',
      });
    })
    .join(' ');
  const lead = t({
    id: 'media.skipped',
    message: plural(skipped.length, { 1: 'Skipped', other: 'Skipped # files.' }),
    comment: 'Before the skipped files and why, e.g. “a.heic, b.pdf: this file type isn’t supported.”',
  });
  return `${lead} ${parts}`;
}

/**
 * "Added 2 clips" once the files are in, with an Undo that takes back this import only while it is still
 * the last change (`undoIf`), never an edit made since.
 */
function sayAdded(count: number, clips: boolean, libraryOnly: boolean) {
  const revision = useProject.getState().snapshot?.history.revision;
  const text = clips
    ? t({
        id: 'media.added.clips',
        message: plural(count, { one: 'Added # clip', other: 'Added # clips' }),
        comment: 'After videos were added to the project',
      })
    : libraryOnly
      ? t({
          id: 'media.added.library',
          message: plural(count, { one: 'Added # file to the library', other: 'Added # files to the library' }),
          comment: 'After music, sounds or pictures were added to the Library (not to the video)',
        })
      : t({
          id: 'media.added.files',
          message: plural(count, { one: 'Added # file', other: 'Added # files' }),
          comment: 'After files (music, images) were added to the project',
        });
  toast.info(text, {
    done: true,
    actions: [
      {
        label: t({ id: 'media.added.undo', message: 'Undo' }),
        run: () =>
          void useProject
            .getState()
            .undoIf((history) => history.revision === revision && history.undoLabel === 'media.import'),
      },
    ],
  });
}

/**
 * Imports files and places them, as one undo step labelled "Import media": `paths` (dropped from Finder), or what the
 * person picks in the file picker (null). With a `target` (files dropped on the timeline) they land there, in order
 * (`dropAllOps`), a sound let go on the Music lane as music that fits the video; without, as the Add button places
 * them: videos at the end of the main track in the order given, sounds and pictures into the library (its tab opens),
 * never the main track. Skipped files are named in one toast. The placing waits for any
 * commit in flight and builds on the project it left. It is kept for the close from the moment it is asked for: a
 * close or quit while the files are read waits for the placing, in its turn.
 */
export async function importFiles(paths: string[] | null, target: DropTarget | null = null): Promise<void> {
  const projectId = useProject.getState().snapshot?.project.id;
  const label = 'media.import';
  const hold = projectId === undefined ? null : keepAtClose(label);
  /** The placing took the hold over (it lets it go at its turn); anything else lets it go here, however it ends. */
  let placing = false;
  try {
    let picked;
    try {
      picked = await bridge.importMedia(paths ?? undefined);
    } catch (error) {
      toast.error(errorMessage(error));
      return;
    }
    // Skipped files are a note when something still came in, and an error only when nothing did.
    if (picked.skipped.length > 0) {
      const notify = picked.assets.length > 0 ? toast.info : toast.error;
      notify(skippedMessage(picked.skipped));
    }
    const { assets } = picked;
    if (assets.length === 0) return;
    // Let go on the Music lane: the first sound is music that fits the video (one step, the file coming in with it).
    const song = target && isMusicTarget(target) ? assets.find((a) => a.kind === 'audio') : undefined;
    if (song && target?.lane === 'sound') {
      await addMusic({ type: 'file', asset: song }, target.atUs);
      return;
    }
    // What goes to the library shows there: its tab opens.
    const shelved = assets.find((a) => a.kind !== 'video');
    if (!target && shelved) useUi.getState().setLibraryTab(tabOf(shelved));
    placing = true;
    // What is still settling goes in first, so the placing builds on it.
    flushBursts();
    afterCommits(
      () => {
        try {
          // The close gave up on it: it never goes in after.
          if (hold?.dropped) return;
          // Not into another project the person opened while the files were read.
          const snapshot = useProject.getState().snapshot;
          if (!snapshot || snapshot.readOnly || snapshot.project.id !== projectId) return;
          const { project } = snapshot;
          const ops = target
            ? dropAllOps(project, assets, target, project.settings.fps)
            : placementOps(assets, project, usePlayback.getState().playheadUs);
          if (ops.length === 0) return;
          // It holds the commit queue until it lands, so an edit made meanwhile builds on the files placed.
          const end = beginCommit();
          void useProject
            .getState()
            .apply(label, ops)
            .then(
              (ok) =>
                ok &&
                sayAdded(
                  assets.length,
                  assets.every((a) => a.kind === 'video'),
                  !target && assets.every((a) => a.kind !== 'video'),
                ),
            )
            .finally(end);
        } finally {
          hold?.release();
        }
      },
      { kept: hold },
    );
  } finally {
    if (!placing) hold?.release();
  }
}

/** Opens the file picker and places what the person picked (videos on the main track, the rest in the library). */
export function useImportMedia(): () => Promise<void> {
  return useCallback(() => importFiles(null), []);
}
