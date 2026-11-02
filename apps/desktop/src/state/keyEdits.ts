import type { Ease } from '../bindings/Ease';
import type { Item } from '../bindings/Item';
import type { Key } from '../bindings/Key';
import type { MotionKeys } from '../bindings/MotionKeys';
import type { Op } from '../bindings/Op';
import {
  LIMITS,
  PROPERTY_KEYS,
  REST,
  VOLUME_LIMITS,
  keyIndexAt,
  landedAt,
  levelAt,
  movedKey,
  movedKeysAt,
  propertyTimes,
  valueAt,
  withKey,
  withMotionKeys,
  withoutKey,
  within,
  type MotionProperty,
} from '../lib/keys';
import { clipOp, itemOp } from '../lib/ops';
import { frameUs } from '../lib/time';
import { clipDurationUs } from '../lib/timeline';
import { clipNow, commitLatest, itemNow, layoutNow } from './latest';
import { useProject } from './project';

/*
 * Edits of keys set by hand, each one undo step built at its turn from the layer or clip as the project has it then
 * (`commitLatest`): a layer's motion keys from the card and the timeline's diamonds, and a clip's or sound's volume keys
 * from the card and the volume line. Times are from the layer's or clip's start.
 */

/** How near the playhead a key counts as at it: half a frame of the project's rate. */
export function keySlackUs(): number {
  const fps = useProject.getState().snapshot?.project.settings.fps ?? 30;
  return Math.round(frameUs(fps) / 2);
}

/** Layer `item`'s motion keys as the project has them now. */
function motionNow(item: Item) {
  return layoutNow(item.id, item.layout).motion;
}

/** The op setting layer `item`'s motion keys to what `change` makes of them now; none when it is gone. */
function motionOps(item: Item, change: Parameters<typeof withMotionKeys>[1]): Op[] {
  const now = itemNow(item.id);
  if (!now) return [];
  return [itemOp(now, { layout: { ...now.layout, motion: withMotionKeys(now.layout.motion, change) } })];
}

/**
 * The key button: a key of `prop` at `tUs` holding the value there (x and y together for Position), or, when it has
 * one there, that key taken away.
 */
export function toggleKey(item: Item, prop: MotionProperty, tUs: number) {
  const slack = keySlackUs();
  const has = propertyTimes(motionNow(item), prop).some((k) => Math.abs(k - tUs) <= slack);
  const label = { id: has ? 'keys.remove.undo' : 'keys.add.undo', params: { property: prop } } as const;
  return commitLatest(label, () =>
    motionOps(item, (keys) =>
      Object.fromEntries(
        PROPERTY_KEYS[prop].map((name) => {
          const list = keys[name];
          if (has) return [name, withoutKey(list, tUs, slack)];
          const v = within(LIMITS[name], valueAt(list, tUs, REST[name]));
          return [name, withKey(list, tUs, v, slack) ?? list];
        }),
      ),
    ),
  );
}

/** The ease into `prop`'s key at `tUs` (both of Position's). */
export function setKeyEase(item: Item, prop: MotionProperty, tUs: number, ease: Ease) {
  const slack = keySlackUs();
  return commitLatest('keys.ease.undo', () =>
    motionOps(item, (keys) =>
      Object.fromEntries(
        PROPERTY_KEYS[prop].map((name) => [
          name,
          keys[name].map((k) => {
            if (Math.abs(k.tUs - tUs) > slack) return k;
            if (ease === 'linear') {
              const plain = { ...k };
              delete plain.ease;
              return plain;
            }
            return { ...k, ease };
          }),
        ]),
      ),
    ),
  );
}

const ALL = Object.keys(REST) as (keyof MotionKeys)[];

/**
 * Every motion key at `fromUs` (a timeline diamond) moved to `toUs`, or copied there (Option drag). Resolves with the
 * time they went to (a move stops short of a key in its way), or null when they didn't go.
 */
export async function moveKeysAt(item: Item, fromUs: number, toUs: number, copy: boolean): Promise<number | null> {
  let landed = null as number | null;
  const ok = await commitLatest(copy ? 'keys.copy.undo' : 'keys.move.undo', () => {
    const len = itemNow(item.id)?.durationUs ?? item.durationUs;
    return motionOps(item, (keys) => {
      const next = movedKeysAt(keys, fromUs, toUs, len, copy);
      landed = landedAt(keys, next);
      return next;
    });
  });
  return ok ? landed : null;
}

/** Every motion key at `tUs` (a timeline diamond) taken away; nothing when the layer has none there by its turn. */
export function deleteKeysAt(item: Item, tUs: number) {
  return commitLatest('keys.delete.undo', () => {
    const keys = motionNow(item)?.keys;
    if (!keys || !ALL.some((name) => (keys[name] ?? []).some((k) => k.tUs === tUs))) return [];
    return motionOps(item, (now) =>
      Object.fromEntries(ALL.map((name) => [name, now[name].filter((k) => k.tUs !== tUs)])),
    );
  });
}

/** Every motion key of layer `id` taken away; its enter, exit and loop stay. */
export function clearMotionKeys(id: string) {
  return commitLatest('keys.clear.undo', () => {
    const now = itemNow(id);
    return now ? motionOps(now, () => ({ x: [], y: [], scale: [], rotation: [], opacity: [] })) : [];
  });
}

/** What carries volume keys: a main-track clip, or a sound layer, by id. */
export interface Leveled {
  kind: 'clip' | 'sound';
  id: string;
}

/** Its keys and volume as the project has them now (none, at full volume, once it is gone). */
export function levelNow(target: Leveled): { keys: Key[]; volume: number; lenUs: number } {
  if (target.kind === 'clip') {
    const c = clipNow(target.id);
    return c ? { keys: c.volumeKeys ?? [], volume: c.volume, lenUs: clipDurationUs(c) } : { keys: [], volume: 1, lenUs: 0 };
  }
  const i = itemNow(target.id);
  const content = i?.content.type === 'audio' ? i.content : null;
  return { keys: content?.volumeKeys ?? [], volume: content?.volume ?? 1, lenUs: i?.durationUs ?? 0 };
}

/** The ops setting its volume keys to `keys` (an empty list takes them off). */
export function volumeKeyOps(target: Leveled, keys: Key[]): Op[] {
  // The last key taken away leaves it at that key's level, not at a volume from before its keys.
  const was = levelNow(target).keys;
  const volume = keys.length === 0 && was.length > 0 ? { volume: was[was.length - 1].v } : {};
  if (target.kind === 'clip') {
    const now = clipNow(target.id);
    return now ? [clipOp(now, { volumeKeys: keys, ...volume })] : [];
  }
  const now = itemNow(target.id);
  if (!now || now.content.type !== 'audio') return [];
  return [itemOp(now, { content: { ...now.content, volumeKeys: keys, ...volume } })];
}

/** Commits what `change` makes of its volume keys now, as `label`; nothing when it is unchanged. */
export function editVolumeKeys(
  label: 'volumeKeys.add.undo' | 'volumeKeys.remove.undo' | 'volumeKeys.move.undo' | 'volumeKeys.clear.undo' | 'volume.undo',
  target: Leveled,
  change: (keys: Key[], volume: number, lenUs: number) => Key[] | null,
) {
  return commitLatest(label, () => {
    const { keys, volume, lenUs } = levelNow(target);
    const next = change(keys, volume, lenUs);
    return next ? volumeKeyOps(target, next) : [];
  });
}

/** A key at `tUs` holding the level there ("Add volume key"), or, with one there, that key taken away. */
export function toggleVolumeKey(target: Leveled, tUs: number) {
  const slack = keySlackUs();
  const has = keyIndexAt(levelNow(target).keys, tUs, slack) >= 0;
  return editVolumeKeys(has ? 'volumeKeys.remove.undo' : 'volumeKeys.add.undo', target, (keys, volume) =>
    has ? withoutKey(keys, tUs, slack) : withKey(keys, tUs, levelAt(keys, tUs, volume), slack),
  );
}

/** A level within the volume's range, on hundredths as the slider sets it. */
export function levelValue(v: number): number {
  return within(VOLUME_LIMITS, Math.round(v * 100) / 100);
}

/**
 * The volume line's key at `fromUs` moved to `toUs` at level `v` (a drag of a point). Resolves with the time it went to
 * (it stops short of a key in its way), or null when it didn't go.
 */
export async function moveVolumeKey(target: Leveled, fromUs: number, toUs: number, v: number): Promise<number | null> {
  let landed = null as number | null;
  const ok = await editVolumeKeys('volumeKeys.move.undo', target, (keys, _volume, lenUs) => {
    // The moved key is the one new object in the list: the others are kept as they were.
    const moved = movedKey(keys, fromUs, toUs, lenUs);
    landed = moved.find((k) => !keys.includes(k))?.tUs ?? null;
    return moved.map((k) => (keys.includes(k) ? k : { ...k, v: levelValue(v) }));
  });
  return ok ? landed : null;
}

/** The volume key at `tUs` taken away; nothing when there is none there by its turn. */
export function deleteVolumeKey(target: Leveled, tUs: number) {
  return editVolumeKeys('volumeKeys.remove.undo', target, (keys) =>
    keys.some((k) => k.tUs === tUs) ? keys.filter((k) => k.tUs !== tUs) : null,
  );
}

/** A key of level `v` at `tUs` (a click on the volume line). */
export function addVolumeKey(target: Leveled, tUs: number, v: number) {
  return editVolumeKeys('volumeKeys.add.undo', target, (keys) => withKey(keys, Math.round(tUs), levelValue(v)));
}
