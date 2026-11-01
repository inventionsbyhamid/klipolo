import type { Clip } from '../../../bindings/Clip';
import type { ErrorParam } from '../../../bindings/ErrorParam';
import type { Recording } from '../../../bindings/Recording';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import type { SourceRole } from '../../../bindings/SourceRole';
import { isVideoRole, makesSound } from '../../../lib/recording';
import {
  klipoloNamed,
  nthParam,
  pictureSoundParam,
  roleParam,
  sourceParam,
  videoSoundParam,
} from '../../../lib/sourceNames';
import type { Sources } from '../../../lib/timeline';

/**
 * A kind of recorded sound the main track plays, one lane each: a role and a place among a recording's sources of that
 * role (`nth`, 0 the first), with each played recording's source there, in the project's order. A picture role's kind
 * is the sound its files carry (a camera's own sound).
 */
export interface Kind {
  role: SourceRole;
  nth: number;
  sources: { recording: Recording; source: RecordingSource }[];
}

/**
 * The kinds of sound the main track's recordings can be heard from (`soundSources`): the sound roles in the order they
 * first come up (the first take's mic first if it lists it first), each role's places in order ("Mic", then "Mic 2"),
 * then the pictures' own sounds ("Camera sound"). Eight takes give one Mic lane, not eight; a recording without a
 * second mic or a sounding camera adds nothing new. Built once per main track and source list.
 * O(clips + recordings × sources).
 */
const kindLists = new WeakMap<Clip[], WeakMap<Sources, Kind[]>>();
export function kindsOf(clips: Clip[], sources: Sources): Kind[] {
  let bySources = kindLists.get(clips);
  if (!bySources) kindLists.set(clips, (bySources = new WeakMap()));
  let out = bySources.get(sources);
  if (!out) {
    const ids = new Set(clips.flatMap((c) => (c.source.type === 'recording' ? [c.source.id] : [])));
    const kinds = new Map<string, Kind>();
    const roles: SourceRole[] = [];
    for (const recording of sources.recordings) {
      if (!ids.has(recording.id)) continue;
      const places = new Map<SourceRole, number>();
      for (const source of recording.sources) {
        const nth = places.get(source.role) ?? 0;
        places.set(source.role, nth + 1);
        if (!makesSound(sources, source)) continue;
        if (!roles.includes(source.role)) roles.push(source.role);
        const key = `${source.role}:${nth}`;
        let kind = kinds.get(key);
        if (!kind) kinds.set(key, (kind = { role: source.role, nth, sources: [] }));
        kind.sources.push({ recording, source });
      }
    }
    const rank = (k: Kind) => (isVideoRole(k.role) ? roles.length : 0) + roles.indexOf(k.role);
    out = [...kinds.values()].sort((a, b) => rank(a) - rank(b) || a.nth - b.nth);
    bySources.set(sources, out);
  }
  return out;
}

/**
 * A kind's lane name as a value (worded in the language shown; undo labels keep it so): a picture's sound is "Camera
 * sound" ("{video}'s sound" for a screen or game file, when every recording's is the same file); a first sound is its
 * sources' own name when they all have the same ("Mic", or the person's "Lav"), else Klipolo's; a second (third...) is
 * the person's name for it when every recording's has the same one, else "Mic 2".
 */
export function laneParam(kind: Kind): ErrorParam {
  const [first, ...rest] = kind.sources;
  const same = rest.every(({ source }) => source.label === first.source.label);
  if (isVideoRole(kind.role)) {
    const base =
      kind.role === 'camera' || same ? pictureSoundParam(first.source) : videoSoundParam(roleParam(kind.role));
    return kind.nth === 0 ? base : nthParam(base, kind.nth);
  }
  if (kind.nth === 0) return same ? sourceParam(first.source) : roleParam(kind.role);
  return same && !klipoloNamed(first.source) ? sourceParam(first.source) : nthParam(roleParam(kind.role), kind.nth);
}
