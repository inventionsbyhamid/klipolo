import type { Asset } from '../bindings/Asset';
import type { CursorStyle } from '../bindings/CursorStyle';
import type { Item } from '../bindings/Item';
import type { Project } from '../bindings/Project';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { Snapshot } from '../bindings/Snapshot';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import type { SourceRole } from '../bindings/SourceRole';
import { DEFAULT_LOOK, preset } from '../lib/defaults';

const S = 1_000_000;

/** The model's default cursor style (`CursorStyle::default`), as a project from Rust carries it. */
export const CURSOR_STYLE: CursorStyle = {
  asRecorded: false,
  hidden: false,
  look: 'recorded',
  size: 1.25,
  smoothing: 'medium',
  click: 'ripple',
  clickColor: '#FFFFFF80',
  motionBlur: 0.5,
};

function asset(
  id: string,
  kind: Asset['kind'],
  seconds: number,
  width: number,
  height: number,
  hasAudio: boolean,
): Asset {
  return {
    id,
    kind,
    path: `/media/${id}`,
    name: `${id}.${kind === 'image' ? 'png' : kind === 'audio' ? 'mp3' : 'mp4'}`,
    durationUs: seconds * S,
    width,
    height,
    fps: kind === 'video' ? 30 : 0,
    hasAudio,
  };
}

export const titleItem: Item = {
  id: 'title',
  startUs: 1 * S,
  durationUs: 3 * S,
  attach: { clipId: 'a', source: { type: 'recording', id: 'rec' }, sourceUs: 1 * S },
  layout: {
    anchor: 'topCenter',
    rect: { x: 0.2, y: 0.08, w: 0.6, h: 0.12 },
    rotation: 0,
    opacity: 1,
    radius: 0,
    overrides: {},
    motion: { enter: preset('fade'), exit: preset('fade') },
  },
  content: {
    type: 'text',
    text: 'Set up billing',
    style: {
      font: { family: 'Inter', source: { type: 'bundled' } },
      size: 72,
      weight: 700,
      italic: false,
      color: '#FFFFFF',
      background: null,
      outline: null,
      shadow: true,
      align: 'center',
    },
  },
  effects: [],
};

/** A recording source of one run, whose file has the source's id. */
export function recSource(
  id: string,
  role: SourceRole,
  offsetUs: number,
  durationUs: number,
): RecordingSource {
  return {
    id,
    role,
    label: `capture.source.${id}`,
    runs: [{ assetId: id, offsetUs, durationUs, driftPpm: 0 }],
    nudgeUs: 0,
    mix: { volume: 1, muted: false, effects: [] },
  };
}

export function fixtureProject(): Project {
  return {
    version: 4,
    id: 'project',
    name: 'Billing tutorial',
    settings: { fps: 30, formats: ['landscape'], primaryFormat: 'landscape' },
    assets: [
      asset('screen', 'video', 60, 1920, 1080, true),
      asset('camera', 'video', 60, 1280, 720, false),
      asset('mic', 'audio', 60, 0, 0, true),
      asset('music', 'audio', 120, 0, 0, true),
      asset('logo', 'image', 0, 512, 512, false),
      asset('broll', 'video', 10, 1920, 1080, true),
    ],
    recordings: [
      {
        id: 'rec',
        kind: 'screen',
        // The camera starts 0.2 s late and ends with the screen, so the take is 60 s long.
        sources: [
          recSource('screen', 'screen', 0, 60 * S),
          recSource('camera', 'camera', 200_000, 59_800_000),
          recSource('mic', 'mic', 0, 60 * S),
        ],
        primary: 'screen',
        markers: [],
      },
    ],
    luts: [],
    timeline: {
      main: [
        {
          id: 'a',
          source: { type: 'recording', id: 'rec' },
          sourceInUs: 0,
          sourceOutUs: 20 * S,
          speed: 1,
          volume: 1,
          muted: false,
          fadeInUs: 0,
          fadeOutUs: 0,
          look: DEFAULT_LOOK,
          camera: {
            mode: 'bubble',
            modeOverrides: { portrait: 'splitBottom' },
            shape: 'circle',
            border: true,
            mirror: false,
            bubble: {
              anchor: 'bottomRight',
              rect: { x: 0.76, y: 0.6044, w: 0.2, h: 0.3556 },
              rotation: 0,
              opacity: 1,
              radius: 0,
              overrides: {},
            },
          },
          framing: {},
          effects: [],
        },
        {
          id: 'b',
          source: { type: 'asset', id: 'broll' },
          sourceInUs: 0,
          sourceOutUs: 10 * S,
          speed: 1,
          volume: 1,
          muted: false,
          fadeInUs: 0,
          fadeOutUs: 0,
          look: DEFAULT_LOOK,
          camera: null,
          framing: {},
          effects: [],
        },
      ],
      tracks: [
        {
          id: 't1',
          kind: 'overlay',
          name: 'Overlay 1',
          hidden: false,
          muted: false,
          locked: false,
          items: [titleItem],
        },
        {
          id: 't2',
          kind: 'audio',
          name: 'Audio 1',
          hidden: false,
          muted: false,
          locked: false,
          items: [
            {
              id: 'song',
              startUs: 0,
              durationUs: 30 * S,
              attach: null,
              layout: {
                anchor: 'center',
                rect: { x: 0, y: 0, w: 1, h: 1 },
                rotation: 0,
                opacity: 1,
                radius: 0,
                overrides: {},
              },
              content: {
                type: 'audio',
                assetId: 'music',
                sourceInUs: 0,
                volume: 0.4,
                fadeInUs: 0,
                fadeOutUs: 0,
                effect: null,
              },
              effects: [],
            },
          ],
        },
      ],
    },
    captions: { enabled: false, place: {}, adSafe: false },
    background: { kind: 'none', color: '#101114', colorEnd: null, assetId: null, padding: 0 },
    audio: { loudness: 'youtube' },
    cursor: CURSOR_STYLE,
    requires: [],
  };
}

export function fixtureSnapshot(): Snapshot {
  return {
    project: fixtureProject(),
    history: { revision: 3, canUndo: true, canRedo: false, undoLabel: 'text.add', redoLabel: null },
  };
}

/**
 * A change that only moves the copy from `revision − 1` to `revision`, for tests whose Rust side (`mockIPC`) answers
 * `project_apply` without changing the project.
 */
export function revisionPatch(revision: number): SnapshotPatch {
  const { history } = fixtureSnapshot();
  return { baseRevision: revision - 1, history: { ...history, revision }, patch: { clips: [], items: [] } };
}

/**
 * A change that turns the project at `snapshot`'s revision into `snapshot.project`, for tests that mock
 * `bridge.apply`, `bridge.undo` or `bridge.redo`. It names every field, so `applyPatch` takes all of them,
 * and it keeps the revision, so repeated calls keep applying as the full snapshots used to.
 */
export function fixturePatch(snapshot: Snapshot): SnapshotPatch {
  const { project, history, readOnly } = snapshot;
  return {
    baseRevision: history.revision,
    history,
    readOnly,
    patch: {
      name: project.name,
      settings: project.settings,
      assets: project.assets,
      recordings: project.recordings,
      luts: project.luts,
      captions: project.captions,
      background: project.background,
      audio: project.audio,
      cursor: project.cursor,
      requires: project.requires,
      main: project.timeline.main.map((c) => c.id),
      clips: project.timeline.main,
      tracks: project.timeline.tracks.map(({ items, ...header }) => ({
        ...header,
        items: items.map((i) => i.id),
      })),
      items: project.timeline.tracks.flatMap((t) => t.items),
    },
  };
}

