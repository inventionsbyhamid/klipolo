import type { Op } from '../bindings/Op';
import type { SnapshotPatch } from '../bindings/SnapshotPatch';
import { labelParts, type Label } from '../lib/labels';
import { afterCommits, beginCommit, keepAtClose } from './commitQueue';
import type { Built } from './editBase';
import { captureBase, useProject } from './project';

/**
 * Sends one labelled change (one undo step) at its turn in the queue (`queueCommit`); a refused edit shows its own
 * toast. `built`: where `ops` were built, when that was earlier (a burst's press). Not exported: every edit a person
 * makes enters the commit queue when it is asked for, never Rust at once, ahead of an edit still on its
 * way that it would then put back (the lint rule on `useProject`'s `apply` holds the rest of the UI to it).
 */
function commit(label: Label, ops: Op[], built?: Built): Promise<boolean> {
  // eslint-disable-next-line no-restricted-syntax -- the commit queue's own sender, at the change's turn
  const { apply } = useProject.getState();
  const [id, params] = labelParts(label);
  if (built) return apply(id, ops, params, built);
  return params === undefined ? apply(id, ops) : apply(id, ops, params);
}

/** A commit through the queue: `landed` says whether it went in. */
export interface QueuedCommit {
  landed: Promise<boolean>;
}

/** An undo label, or a function that words it as the change goes in, just before its ops are built (a toggle's). */
export type LateLabel = Label | (() => Label);

/**
 * A change the close waits for instead of dropping it while it waits its turn (`keepAtClose`: every burst's and
 * one-shot press's), with the label said if the close can't wait for it.
 */
export interface KeepAtClose {
  keep?: Label;
}

/**
 * One undo step whose ops `build` makes when it goes in: at once when no commit is on its way, else once those have
 * landed (`afterCommits`), so ops that carry a whole value (a layer's content, a point) build on a change another
 * control has just made. It holds the commit queue until it lands, so the next one (an edit, undo, redo, an export)
 * comes after it and builds on it. `landed` resolves with whether it went in: false when `build` throws (logged). If
 * the project closes while it waits, a `keep` one goes in its turn, the close waiting for it (`keepAtClose`), and false
 * if the close gives up on it (2 s; it is said); any other is dropped and never resolves. Its base is taken when it is
 * asked for (or `built`, a burst's, at its press): its builder closes over that moment's values, so a change from
 * elsewhere merged while it waits makes Rust refuse it; this window's own commits landing meanwhile don't.
 */
export function queueCommit(
  label: LateLabel,
  build: () => Op[],
  built: Built = captureBase(),
  { keep }: KeepAtClose = {},
): QueuedCommit {
  let resolve!: (ok: boolean) => void;
  const landed = new Promise<boolean>((r) => (resolve = r));
  const hold = keep === undefined ? null : keepAtClose(keep);
  if (hold) hold.drop = () => resolve(false);
  afterCommits(
    () => {
      const end = beginCommit();
      const worded = typeof label === 'function' ? label() : label;
      if (hold) {
        // Under way: it lands ahead of the close in any case; said as it goes, if ever said.
        hold.started = true;
        hold.label = worded;
      }
      // A builder that throws becomes a refusal here, so the queue is released (`finally`) and later edits still go.
      void new Promise<boolean>((sent) => sent(commit(worded, build(), built)))
        .then(resolve, (error: unknown) => {
          console.error(error);
          resolve(false);
        })
        .finally(() => {
          end();
          hold?.release();
        });
    },
    { kept: hold },
  );
  return { landed };
}

/**
 * One undo step built when it goes in, through the commit queue (`queueCommit`); resolves with whether it went in. With
 * a plain label it is kept at close (every one-shot press is, as every burst is): the close waits for it in
 * its turn, and says it (by that label) if it can't. `keep: false` lets the close drop it instead; a late (function)
 * label can't be kept, having nothing to say before it goes.
 */
export function commitLatest(label: Label, build: () => Op[], opts?: { keep: boolean }): Promise<boolean>;
export function commitLatest(label: LateLabel, build: () => Op[]): Promise<boolean>;
export function commitLatest(
  label: LateLabel,
  build: () => Op[],
  opts?: { keep: boolean },
): Promise<boolean> {
  const keep = opts?.keep !== false && typeof label !== 'function' ? label : undefined;
  return queueCommit(label, build, undefined, { keep }).landed;
}

/**
 * One undo step Rust builds (a media command), sent at its turn in the commit queue: after every commit on its way, so
 * it builds on them, holding the queue until it lands. Its base is taken now, when it is asked for. Resolves with
 * whether it went in; a refusal shows its own toast.
 */
export function queueCommand(task: () => Promise<SnapshotPatch>): Promise<boolean> {
  const built = captureBase();
  return new Promise((resolve) => {
    afterCommits(() => {
      const end = beginCommit();
      void useProject
        .getState()
        .send(task, built)
        .then(resolve, () => resolve(false))
        .finally(end);
    });
  });
}

/** Each switch's presses kept at close that haven't landed, by its target and two labels: what the next will do. */
const togglesAhead = new Map<string, number>();

/**
 * A switch's one undo step, decided as it goes in from what `latest()` says then, so two quick presses turn it on and
 * back off. `label(on)` and `ops(on)` leave it `on`. Resolves with the state it set, or null when it didn't go in.
 * Kept at close unless `keep: false`: the close waits for it; if it can't, it is said as it would go
 * in: after the presses ahead of it that haven't landed, so two quick presses read "on", then "off".
 * `id`: what it switches (a clip, a layer), when switches of several share their labels (Mute clip): a press of
 * another's is never counted ahead of it.
 */
export async function commitToggle(
  latest: () => boolean,
  label: (on: boolean) => Label,
  ops: (on: boolean) => Op[],
  { keep = true, id }: { keep?: boolean; id?: string } = {},
): Promise<boolean | null> {
  let on = false;
  const key = JSON.stringify([id ?? null, label(true), label(false)]);
  const ahead = keep ? (togglesAhead.get(key) ?? 0) : 0;
  if (keep) togglesAhead.set(key, ahead + 1);
  // How the switch reads once the presses ahead of it have landed: this one turns it the other way.
  const before = latest() !== (ahead % 2 === 1);
  const landed = queueCommit(
    () => label((on = !latest())),
    () => ops(on),
    undefined,
    { keep: keep ? label(!before) : undefined },
  ).landed;
  const ok = await landed.finally(() => {
    if (!keep) return;
    const left = (togglesAhead.get(key) ?? 1) - 1;
    if (left > 0) togglesAhead.set(key, left);
    else togglesAhead.delete(key);
  });
  return ok ? on : null;
}

