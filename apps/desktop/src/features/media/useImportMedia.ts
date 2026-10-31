import { plural, t } from '@lingui/core/macro';
import { useCallback } from 'react';
import type { Asset } from '../../bindings/Asset';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import type { SkippedFile } from '../../bindings/SkippedFile';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { bridge } from '../../bridge/commands';
import { library } from '../../bridge/library';
import { errorMessage } from '../../lib/errors';
import { placementOps } from '../../lib/placement';
import { flushBursts } from '../../state/bursts';
import { afterCommits, beginCommit, keepAtClose } from '../../state/commitQueue';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { useUi, type LibraryTab } from '../../state/ui';
import { dropAllOps, type DropTarget } from '../timeline/drops';
import { dropOnto, dropUs, isMusicTarget } from './place';

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

/** "Added lofi.mp3 as music", and where the other files dropped with it went. */
function sayMusicAdded(song: Asset, rest: Asset[]) {
  const name = song.name;
  const others = rest.length;
  const text =
    others === 0
      ? t({ id: 'media.added.music.only', message: `Added ${name} as music` })
      : rest.every((a) => a.kind !== 'video')
        ? t({
            id: 'media.added.music.library',
            message: plural(others, {
              one: `Added ${name} as music, # file to the library`,
              other: `Added ${name} as music, # files to the library`,
            }),
            comment: '{name} is the file let go on the Music lane; the others dropped with it went to the Library',
          })
        : t({
            id: 'media.added.music.more',
            message: plural(others, {
              one: `Added ${name} as music and # more file`,
              other: `Added ${name} as music and # more files`,
            }),
            comment: '{name} is the file let go on the Music lane; the others went where Add files puts them',
          });
  toast.info(text, { done: true });
}

/**
 * The undo steps that place freshly imported `assets`, each sent at the placing's turn, one after another: `music` (a
 * sound let go on the Music lane) as music from there; then, let go on another lane (`target`), the videos as
 * `dropAllOps` puts them and the sounds and pictures as the library's "+" does (`library.place`, on the track let go
 * on); with no lane, all of them where the Add button puts them (`placementOps`).
 */
function placeSteps(
  project: Project,
  assets: Asset[],
  target: DropTarget | null,
  music: { song: Asset; target: DropTarget } | null,
): (() => Promise<boolean>)[] {
  const send = (task: () => Promise<SnapshotPatch>) => () => useProject.getState().send(task);
  const apply = (ops: Op[]) => () =>
    ops.length === 0 ? Promise.resolve(true) : useProject.getState().apply('media.import', ops);
  const steps: (() => Promise<boolean>)[] = [];
  if (music) {
    const from = dropUs(project, music.target);
    steps.push(send(() => library.addMusic({ type: 'file', asset: music.song }, from)));
  }
  if (!target) {
    if (assets.length > 0) steps.push(apply(placementOps(assets)));
    return steps;
  }
  const videos = assets.filter((a) => a.kind === 'video');
  const others = assets.filter((a) => a.kind !== 'video');
  if (videos.length > 0) steps.push(apply(dropAllOps(project, videos, target, project.settings.fps)));
  if (others.length > 0) {
    const at = dropUs(project, target);
    const onto = dropOnto(target);
    steps.push(send(() => library.place(others.map((asset) => ({ type: 'file', asset })), at, onto)));
  }
  return steps;
}

/**
 * Imports files and places them (`placeSteps`): `paths` (dropped from Finder), or what the person picks in the file
 * picker (null). With a `target` (files dropped on the timeline) they land there, in order: videos as `dropAllOps`
 * puts them, sounds and pictures as the library's "+" does, and a sound let go on the Music lane as music that fits the
 * video (the rest of that drop going where the Add button puts it); without, as the Add button places them: videos at
 * the end of the main track in the order given, sounds and pictures into the library (its tab opens), never the main
 * track. Skipped files are named in one toast. The placing waits for any
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
    // Let go on the Music lane: the first sound is music that fits the video, and the rest go where the Add button puts
    // them. Elsewhere on the timeline videos go in as dropped there, and sounds and pictures as the "+" places them.
    const song = target && isMusicTarget(target) ? assets.find((a) => a.kind === 'audio') : undefined;
    const rest = song ? assets.filter((a) => a !== song) : assets;
    const lane = song ? null : target;
    // What goes to the library shows there: its tab opens.
    const shelved = rest.find((a) => a.kind !== 'video');
    if (!lane && shelved) useUi.getState().setLibraryTab(tabOf(shelved));
    placing = true;
    // What is still settling goes in first, so the placing builds on it.
    flushBursts();
    afterCommits(
      () => {
        if (hold) hold.started = true;
        // The close gave up on it: it never goes in after.
        if (hold?.dropped) return hold.release();
        // Not into another project the person opened while the files were read.
        const snapshot = useProject.getState().snapshot;
        if (!snapshot || snapshot.readOnly || snapshot.project.id !== projectId) return hold?.release();
        const { project } = snapshot;
        const steps = placeSteps(project, rest, lane, song && target ? { song, target } : null);
        if (steps.length === 0) return hold?.release();
        // It holds the commit queue until every step lands, so an edit made meanwhile builds on the files placed.
        const end = beginCommit();
        void (async () => {
          let ok = true;
          for (const step of steps) ok = (await step().catch(() => false)) && ok;
          if (!ok) return;
          if (song) sayMusicAdded(song, rest);
          else
            sayAdded(
              assets.length,
              assets.every((a) => a.kind === 'video'),
              !target && assets.every((a) => a.kind !== 'video'),
            );
        })().finally(() => {
          end();
          hold?.release();
        });
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
