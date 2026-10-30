import { plural, t } from '@lingui/core/macro';
import { useCallback } from 'react';
import type { SkippedFile } from '../../bindings/SkippedFile';
import { bridge } from '../../bridge/commands';
import { errorMessage } from '../../lib/errors';
import { placementOps } from '../../lib/placement';
import { flushBursts } from '../../state/bursts';
import { afterCommits, beginCommit, keepAtClose } from '../../state/commitQueue';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';

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
  return t({
    id: 'media.skipped.namesMore',
    message: plural(more, { one: `${shown} and # more`, other: `${shown} and # more` }),
    comment: '{shown} lists the first file names, e.g. “a.heic, b.pdf, c.zip”',
  });
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
  const count = skipped.length;
  return t({
    id: 'media.skipped',
    message: plural(count, { 1: `Skipped ${parts}`, other: `Skipped # files. ${parts}` }),
    comment: '{parts} names the skipped files and why, e.g. “a.heic, b.pdf: this file type isn’t supported.”',
  });
}

/**
 * Imports files and places them, as one undo step labelled "Import media": `paths` (dropped from Finder), or what the
 * person picks in the file picker (null). They land as the Add button places them: videos at the end of the main
 * track in the order given, music on its own track, images at the playhead. Skipped files are named in one toast. The
 * placing waits for any commit in flight and builds on the project it left. It is kept for the close from the moment
 * it is asked for: a close or quit while the files are read waits for the placing, in its turn.
 */
export async function importFiles(paths: string[] | null): Promise<void> {
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
          const ops = placementOps(assets, snapshot.project, usePlayback.getState().playheadUs);
          if (ops.length === 0) return;
          // It holds the commit queue until it lands, so an edit made meanwhile builds on the files placed.
          const end = beginCommit();
          void useProject.getState().apply(label, ops).finally(end);
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

/** Opens the file picker and places what the person picked (videos on the main track, music on its own track). */
export function useImportMedia(): () => Promise<void> {
  return useCallback(() => importFiles(null), []);
}
