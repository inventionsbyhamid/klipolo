import { t } from '@lingui/core/macro';
import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { ClipSource } from '../bindings/ClipSource';
import type { Item } from '../bindings/Item';
import type { ItemContent } from '../bindings/ItemContent';
import type { Project } from '../bindings/Project';
import type { Timeline } from '../bindings/Timeline';
import type { Track } from '../bindings/Track';
import { codedMessage } from '../bridge/call';
import { activeIntlLocale } from '../i18n';
import { agentName } from './agentNames';
import { contribLabel, isInstalled, isScene } from './contrib';
import { firstAsset, primarySource, soundSources } from './recording';
import { formatNumber } from './format';
import { showsPictures, soloOf } from './solo';
import { US_PER_SEC } from './time';

export type LayerKind = ItemContent['type'];

/** How long `sourceUs` of footage plays at `speed`, to the µs (the engine's rounding). */
export function playedUs(sourceUs: number, speed: number): number {
  return Math.round(sourceUs / speed);
}

export function clipDurationUs(clip: Pick<Clip, 'sourceInUs' | 'sourceOutUs' | 'speed'>): number {
  return playedUs(clip.sourceOutUs - clip.sourceInUs, clip.speed);
}

export function clipStarts(main: Clip[]): number[] {
  let t = 0;
  return main.map((clip) => {
    const start = t;
    t += clipDurationUs(clip);
    return start;
  });
}

/** The main track's own length: its clips back to back. The video may run longer (`videoDurationUs`). */
export function mainDurationUs(main: Clip[]): number {
  return main.reduce((sum, clip) => sum + clipDurationUs(clip), 0);
}

/**
 * The kinds with a picture of their own, as Rust's exhaustive match lists them (a positive list: a new kind counts only
 * once it is added here).
 */
const OWN_PICTURE: ReadonlySet<ItemContent['type']> = new Set(['video', 'image', 'text', 'generator']);

/**
 * Layers with a picture of their own: not sound or zooms, and not an adjustment box, which only changes what is below
 * it. They set the video's length. Mirrors `ItemContent::has_own_picture`.
 */
export function hasOwnPicture(content: ItemContent): boolean {
  return OWN_PICTURE.has(content.type);
}

const mainEnds = new WeakMap<Clip[], number>();
const layerEnds = new WeakMap<Track[], number>();

/** `mainDurationUs`, worked out once per main track array (patches keep an unchanged one). */
function mainEndUs(main: Clip[]): number {
  let end = mainEnds.get(main);
  if (end === undefined) {
    end = mainDurationUs(main);
    mainEnds.set(main, end);
  }
  return end;
}

/** Where the last layer with a picture of its own ends (0 without one), once per tracks array (patches keep it). */
function layersEndUs(tracks: Track[]): number {
  let end = layerEnds.get(tracks);
  if (end === undefined) {
    end = 0;
    for (const track of tracks) {
      for (const item of track.items) if (hasOwnPicture(item.content)) end = Math.max(end, itemEndUs(item));
    }
    layerEnds.set(tracks, end);
  }
  return end;
}

/**
 * How long the video is: until the main track or its last layer with a picture of its own ends, whichever is later,
 * so a project of layers alone has a length. Sound layers, zooms and adjustment boxes never make it
 * longer; layers on a hidden or soloed-away track count (hiding is a view switch). Mirrors
 * `klipolo_model::query::video_duration`. O(clips + layers) once per main and tracks array (patches keep unchanged
 * ones), then O(1).
 */
export function videoDurationUs(timeline: Pick<Timeline, 'main' | 'tracks'>): number {
  return Math.max(mainEndUs(timeline.main), layersEndUs(timeline.tracks));
}

export function clipAt(main: Clip[], at: number): { clip: Clip; index: number; startUs: number } | null {
  let start = 0;
  for (let index = 0; index < main.length; index++) {
    const clip = main[index];
    const end = start + clipDurationUs(clip);
    if (at >= start && at < end) return { clip, index, startUs: start };
    start = end;
  }
  return null;
}

export function itemEndUs(item: Item): number {
  return item.startUs + item.durationUs;
}

/**
 * When `item` is first drawn whole: once its enter preset has run (every preset starts hidden or off-canvas),
 * otherwise at its start. Mirrors the engine's `shown_from_us`, pinned by `layout-parity.json`.
 */
export function shownFromUs(item: Item): number {
  const enter = item.layout.motion?.enter;
  return enter === undefined ? item.startUs : item.startUs + Math.min(enter.durationUs, item.durationUs);
}

/**
 * Where to put the playhead to show `item`: the first frame on which it is drawn whole. A paused preview at a title's
 * own start shows the first frame of its fade-in, which is fully transparent, so a new or picked title looked missing.
 */
export function showItemAtUs(item: Item, fps: number): number {
  const shown = shownFromUs(item);
  if (shown === item.startUs || fps <= 0) return shown;
  const frame = Math.ceil((shown * fps) / US_PER_SEC - 1e-6);
  const onGrid = Math.round((frame * US_PER_SEC) / fps);
  return onGrid < itemEndUs(item) ? onGrid : shown;
}

/** Visible overlay layers at `at`, bottom to top (a track solo leaves out isn't drawn). */
export function visualItemsAt(project: Project, at: number): Item[] {
  const solo = soloOf(project.timeline.tracks);
  return project.timeline.tracks
    .filter((track) => showsPictures(solo, track))
    .flatMap((track) => track.items.filter((item) => at >= item.startUs && at < itemEndUs(item)));
}

export function findItem(project: Project, id: string): { item: Item; track: Track } | null {
  for (const track of project.timeline.tracks) {
    const item = track.items.find((candidate) => candidate.id === id);
    if (item) return { item, track };
  }
  return null;
}

export function findClip(project: Project, id: string): Clip | null {
  return project.timeline.main.find((clip) => clip.id === id) ?? null;
}

/** What a block's label is looked up in (arrays a patch keeps when they don't change, so lanes can be `memo`). */
export type Sources = Pick<Project, 'assets' | 'recordings'>;

export function assetById(project: Pick<Project, 'assets'>, id: string | null | undefined): Asset | null {
  return project.assets.find((asset) => asset.id === id) ?? null;
}

/**
 * Whether layer `item` plays sound of its own, which its Mute silences: a sound layer, or a video layer whose file has
 * sound. The card, the block menu and the palette offer Mute only then.
 */
export function hasOwnSound(project: Pick<Project, 'assets'>, item: Item): boolean {
  const content = item.content;
  return content.type === 'audio' || (content.type === 'video' && !!assetById(project, content.assetId)?.hasAudio);
}

/**
 * Whether main clip `clip` is a freeze frame (`Clip.freezeAtUs`): it holds that one frame of its source for its length,
 * silent; its source range only gives the length.
 */
export function isFreeze(clip: Pick<Clip, 'freezeAtUs'>): boolean {
  return clip.freezeAtUs !== undefined && clip.freezeAtUs !== null;
}

/**
 * How a freeze frame's block reads its footage: from its frame, at a speed so near 0 that every point of the block shows
 * that frame (a speed of 0 would divide by 0 where a block maps its pixels back to source time).
 */
export const STILL_SPEED = 1e-9;

/** Whether main clip `clip` has sound for its Volume and Mute: its file's, or its recording's sound sources. */
export function clipHasSound(project: Sources, clip: Clip): boolean {
  // A freeze frame plays none.
  if (isFreeze(clip)) return false;
  const source = clip.source;
  if (source.type === 'asset') return !!assetById(project, source.id)?.hasAudio;
  const recording = project.recordings.find((r) => r.id === source.id);
  return !!recording && soundSources(project, recording).length > 0;
}

/**
 * Whether imported clip `main[i]`'s own sound can go on a layer of its own (Separate sound, `detachSound`): 'none' for a
 * recording clip (its sounds go one by one), a freeze frame or a file without sound; 'sped' at a speed other than 1×,
 * which the model refuses; 'separated' while a sound layer of its file plays over it; else 'separable'. O(clips +
 * layers).
 */
export function clipSoundState(
  project: Pick<Project, 'assets' | 'timeline'>,
  i: number,
): 'none' | 'sped' | 'separated' | 'separable' {
  const clip = project.timeline.main[i];
  const source = clip?.source;
  if (!clip || source.type !== 'asset' || isFreeze(clip) || !assetById(project, source.id)?.hasAudio) return 'none';
  if (clip.speed !== 1) return 'sped';
  const startUs = clipStarts(project.timeline.main)[i];
  const endUs = startUs + clipDurationUs(clip);
  const heard = project.timeline.tracks.some((track) =>
    track.items.some(
      (item) =>
        item.content.type === 'audio' &&
        item.content.assetId === source.id &&
        item.startUs < endUs &&
        itemEndUs(item) > startUs,
    ),
  );
  return heard ? 'separated' : 'separable';
}

/**
 * The names below are in the active language: a component that shows them re-renders on a language switch only if it
 * reads the language itself (`useLingui`, `useLocale`).
 */
const missingMedia = () =>
  t({ id: 'layer.missingMedia', message: 'Missing media', comment: 'A clip whose file is gone' });

/** The file a clip plays: its asset, or a recording's main footage (its screen, else its camera; its first file).
 * Null when it's gone. */
export function primaryAsset(project: Sources, source: ClipSource): Asset | null {
  if (source.type === 'asset') return assetById(project, source.id);
  const recording = project.recordings.find((r) => r.id === source.id);
  return assetById(project, recording ? firstAsset(primarySource(recording)) : null);
}

/**
 * A recording's name, the one Klipolo gives it wherever it shows (its clips on the timeline and in the clip card, the
 * Library, the toast that said it landed, export's notes): "Recording 2" for the project's second, by its place among
 * the project's recordings (the model's `name.recording`). "Recording" when the project doesn't have it.
 */
export function recordingName(project: Pick<Project, 'recordings'>, recordingId: string): string {
  const number = project.recordings.findIndex((r) => r.id === recordingId) + 1;
  const plain = t({ id: 'layer.recording', message: 'Recording', comment: 'A clip recorded in Klipolo' });
  return (number > 0 && codedMessage({ code: 'name.recording', params: { number } })) || plain;
}

/**
 * A clip's name: a recording's own ("Recording 1", never its screen file's), else its file's as the Library names it
 * ("Screen of Recording 1" for a recording's file added on its own).
 */
export function sourceLabel(project: Sources, source: ClipSource): string {
  if (source.type === 'recording') return recordingName(project, source.id);
  const asset = primaryAsset(project, source);
  return asset ? assetName(project, asset) : missingMedia();
}

/** A main clip's name: its source's ("broll.mp4", "Recording 1"), a freeze frame's of it ("Freeze frame of …"). */
export function clipLabel(project: Sources, clip: Clip): string {
  const name = sourceLabel(project, clip.source);
  if (!isFreeze(clip)) return name;
  return t({
    id: 'clip.freezeName',
    message: `Freeze frame of ${name}`,
    comment: 'A freeze frame on the timeline and its card; {name} is the clip it holds a frame of, e.g. intro.mov',
  });
}

/**
 * A file's name in the Library and on the timeline: a recording's files by their source and their recording ("Screen
 * of Recording 1", "Camera of Recording 1"), as each plays that source alone (only the recording's own clip is
 * "Recording 1"); any other file by its own name.
 */
export function assetName(project: Sources, asset: Asset): string {
  for (const recording of project.recordings) {
    const source = recording.sources.find((s) => s.runs.some((run) => run.assetId === asset.id));
    if (!source) continue;
    const name = recordingName(project, recording.id);
    const role = codedMessage({ code: source.label, params: {} }) || source.label;
    return t({
      id: 'library.recordingFile',
      message: `${role} of ${name}`,
      comment: 'A file of a recording made in Klipolo: {role} is Screen, Camera or Mic, {name} is Recording 1',
    });
  }
  return asset.name;
}

export function itemLabel(project: Sources, item: Item): string {
  const content = item.content;
  switch (content.type) {
    case 'text':
      return content.text;
    case 'zoom': {
      const scale = formatNumber(activeIntlLocale(), content.scale);
      return t({
        id: 'layer.zoom.label',
        message: `Zoom ${scale}×`,
        comment: 'A zoom layer; {scale} is e.g. 1.5',
      });
    }
    case 'generator':
      // A motion graphic is named as one, by its id, never by a layer's name.
      return contribLabel(content.source, isScene(content.source) ? 'scene' : 'generator');
    case 'adjustment':
      return t({ id: 'layer.adjustment', message: 'Adjustment', comment: 'An adjustment layer' });
    default: {
      const asset = assetById(project, content.assetId);
      return asset ? assetName(project, asset) : missingMedia();
    }
  }
}

/**
 * A tooltip for a layer whose contribution isn't installed: its id (the label says "Missing generator", or "Missing
 * motion graphic" for a scene).
 */
export function itemTitle(item: Item): string | undefined {
  const content = item.content;
  return content.type === 'generator' && !isInstalled(content.source) && agentName(content.source) === null
    ? content.source
    : undefined;
}
