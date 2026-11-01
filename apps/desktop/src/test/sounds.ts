import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Item } from '../bindings/Item';
import type { Project } from '../bindings/Project';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { Snapshot } from '../bindings/Snapshot';
import type { SourceRole } from '../bindings/SourceRole';
import { DEFAULT_LOOK } from '../lib/defaults';
import { fixtureProject } from './fixtures';

const S = 1_000_000;

function file(id: string, kind: 'video' | 'audio', seconds: number, hasAudio: boolean): Asset {
  return {
    id,
    kind,
    path: `/media/${id}`,
    name: `${id}.${kind === 'audio' ? 'm4a' : 'mp4'}`,
    durationUs: seconds * S,
    width: kind === 'video' ? 1920 : 0,
    height: kind === 'video' ? 1080 : 0,
    fps: kind === 'video' ? 30 : 0,
    hasAudio,
  };
}

/** A source of one run over the whole take, whose file is `assetId`, named as Klipolo names its role. */
function source(id: string, role: SourceRole, assetId: string, seconds: number, label = id): RecordingSource {
  return {
    id,
    role,
    label: `capture.source.${label}`,
    runs: [{ assetId, offsetUs: 0, durationUs: seconds * S, driftPpm: 0 }],
    nudgeUs: 0,
    mix: { volume: 1, muted: false, effects: [] },
  };
}

function clip(id: string, recording: string, fromS: number, toS: number): Clip {
  return {
    id,
    source: { type: 'recording', id: recording },
    sourceInUs: fromS * S,
    sourceOutUs: toS * S,
    speed: 1,
    volume: 1,
    muted: false,
    fadeInUs: 0,
    fadeOutUs: 0,
    look: DEFAULT_LOOK,
    camera: null,
    framing: {},
    effects: [],
  };
}

/** A recorded sound on a layer of its own, `fromS` to `toS` on the timeline, tied to its source. */
export function separated(id: string, assetId: string, fromS: number, toS: number, tie?: [string, string]): Item {
  return {
    id,
    startUs: fromS * S,
    durationUs: (toS - fromS) * S,
    attach: null,
    layout: { anchor: 'center', rect: { x: 0, y: 0, w: 1, h: 1 }, rotation: 0, opacity: 1, radius: 0, overrides: {} },
    content: {
      type: 'audio',
      assetId,
      sourceInUs: fromS * S,
      volume: 1,
      fadeInUs: 0,
      fadeOutUs: 0,
      effect: null,
      ...(tie && { tie: { recording: tie[0], source: tie[1] } }),
    },
    effects: [],
  };
}

/**
 * Two takes. r1 (screen, mic, computer sound; 60 s) plays as c1 (0 to 20 s) and c2 (20 to 40 s); r2 (screen and mic;
 * 30 s) as c3. c1's mic is separated onto a sound layer of its own (its clip mix muted, as separating leaves it), and
 * c2 plays the mic at 60 % of its own. No take's screen file carries sound, as a Klipolo take's doesn't.
 */
export function twoTakes(): Project {
  const project = fixtureProject();
  project.assets = [
    file('v1', 'video', 60, false),
    file('m1', 'audio', 60, true),
    file('s1', 'audio', 60, true),
    file('v2', 'video', 30, false),
    file('m2', 'audio', 30, true),
  ];
  project.recordings = [
    {
      id: 'r1',
      kind: 'screen',
      sources: [
        source('screen', 'screen', 'v1', 60),
        source('mic', 'mic', 'm1', 60),
        source('computer', 'systemAudio', 's1', 60, 'computerSound'),
      ],
      primary: 'screen',
      markers: [],
    },
    {
      id: 'r2',
      kind: 'screen',
      sources: [source('screen', 'screen', 'v2', 30), source('mic', 'mic', 'm2', 30)],
      primary: 'screen',
      markers: [],
    },
  ];
  const c1 = clip('c1', 'r1', 0, 20);
  c1.sourceMix = { mic: { volume: 1, muted: true, effects: [] } };
  const c2 = clip('c2', 'r1', 20, 40);
  c2.sourceMix = { mic: { volume: 0.6, muted: false, effects: [] } };
  project.timeline = {
    main: [c1, c2, clip('c3', 'r2', 0, 30)],
    tracks: [
      {
        id: 't1',
        kind: 'audio',
        name: 'Audio 1',
        hidden: false,
        muted: false,
        locked: false,
        items: [separated('c1-mic', 'm1', 0, 20, ['r1', 'mic'])],
      },
    ],
  };
  return project;
}

export function twoTakesSnapshot(): Snapshot {
  return {
    project: twoTakes(),
    history: { revision: 3, canUndo: true, canRedo: false, undoLabel: null, redoLabel: null },
  };
}
