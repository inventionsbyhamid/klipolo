import type { ChatOutcome } from '../bindings/ChatOutcome';
import type { ChatRequest } from '../bindings/ChatRequest';
import type { Op } from '../bindings/Op';
import type { RangeUs } from '../bindings/RangeUs';
import { installChatBridge, tauriChat } from '../bridge/chat';
import { fakeChat, proposal, reply } from '../bridge/chatFake';
import { bridge } from '../bridge/commands';
import { speech } from '../bridge/speech';
import { mainIndex } from '../features/timeline/model/index';
import { errorMessage } from '../lib/errors';
import { useChatStandIn } from '../state/chat';
import { useProject } from '../state/project';
import { toast } from '../state/toasts';

/**
 * Development builds only, until the planner's commands are in the app and analysis has run on a clip: Control Shift P
 * turns a stand-in chat on or off, which answers a few requests the way the planner does (a proposal of ums and pauses,
 * captions turned on, moments, keep only these) through the app's own project commands, so the panel, the red ranges,
 * Apply and Undo can be checked by hand (while it is on, chat runs without a key or code). Control Shift W gives the open
 * project's clips sample words.
 */

const S = 1_000_000;

/** The proposals the stand-in made, by id: what Apply cuts. */
const made = new Map<string, RangeUs[]>();

function mainEnd(): number {
  const main = useProject.getState().snapshot?.project.timeline.main;
  return main ? mainIndex(main).mainEndUs : 0;
}

/** An "um" or a pause every few seconds over the open project's clips. */
function cutsOf(end: number): { ranges: RangeUs[]; tags: { startUs: number; endUs: number; label: string }[] } {
  const ranges: RangeUs[] = [];
  const tags = [];
  for (let at = 3 * S, k = 0; at + 1.5 * S < end; at += 6.5 * S, k++) {
    const pause = k % 3 === 1;
    const range = { start: at, end: at + (pause ? 1.4 : 0.4) * S };
    ranges.push(range);
    tags.push({ startUs: range.start, endUs: range.end, label: pause ? 'pause' : 'um' });
  }
  return { ranges, tags };
}

/** What isn't in `keep` of `[0, end)`. */
function outside(keep: RangeUs[], end: number): RangeUs[] {
  const out: RangeUs[] = [];
  let at = 0;
  for (const r of [...keep].sort((a, b) => a.start - b.start)) {
    if (r.start > at) out.push({ start: at, end: r.start });
    at = Math.max(at, r.end);
  }
  if (at < end) out.push({ start: at, end });
  return out;
}

const length = (ranges: RangeUs[]) => ranges.reduce((n, r) => n + r.end - r.start, 0);

async function answer(request: ChatRequest): Promise<ChatOutcome> {
  const snapshot = useProject.getState().snapshot;
  if (!snapshot) return reply(request.id, 'Open a project first.');
  const end = mainEnd();
  const revision = snapshot.history.revision;
  const text = request.text.toLowerCase();
  if (request.keepOnly.length > 0) {
    const ranges = outside(request.keepOnly, end);
    const id = `dev-${crypto.randomUUID()}`;
    made.set(id, ranges);
    const p = proposal(id, revision, {
      title: `Keep only ${request.keepOnly.length} moments`,
      removedUs: length(ranges),
      removals: { ...proposal('x', 0).removals, ranges },
    });
    return reply(request.id, 'Everything else is marked in red on the timeline.', { proposal: p });
  }
  if (/\bums?\b|pause|filler/.test(text)) {
    const { ranges, tags } = cutsOf(end);
    const pauses = tags.filter((t) => t.label === 'pause').length;
    const ums = tags.length - pauses;
    const id = `dev-${crypto.randomUUID()}`;
    made.set(id, ranges);
    const p = proposal(id, revision, {
      title: `Cut ${pauses} pauses and ${ums} ums`,
      removedUs: length(ranges),
      removals: { ...proposal('x', 0).removals, ranges },
      tags,
    });
    return reply(
      request.id,
      `I found ${pauses} pauses longer than 0.8 s and ${ums} ums. They're marked in red on the timeline.`,
      { proposal: p },
    );
  }
  if (/caption/.test(text)) {
    const captions = { ...snapshot.project.captions, enabled: true, style: 'klipolo.builtin/caption.clean' };
    const change = await bridge.apply({ label: 'captions.on.undo', ops: [{ type: 'setCaptions', captions }] });
    return reply(
      request.id,
      'Captions are on, in the Clean style. Pick another style or fix words in the Captions panel.',
      { applied: { change, undo: { code: 'captions.on.undo', params: {} } } },
    );
  }
  if (/moment|find|highlight/.test(text)) {
    const at = (share: number) => Math.round(end * share);
    return reply(request.id, 'Here are 3 moments worth a look.', {
      moments: [
        { startUs: at(0.1), endUs: at(0.18), label: 'The first firing' },
        { startUs: at(0.42), endUs: at(0.5), label: 'Cones and timing' },
        { startUs: at(0.7), endUs: at(0.78), label: 'The cracked bowl' },
      ],
    });
  }
  return reply(
    request.id,
    'This is a stand-in for chat in development. Try: cut the ums and long pauses, add captions, or find the best moments.',
  );
}

const devChat = fakeChat({
  reply: answer,
  apply: async (id) => {
    const ranges = made.get(id) ?? [];
    const ops: Op[] = [{ type: 'deleteRanges', ranges }];
    const change = await bridge.apply({ label: 'chat.removeFillers', ops });
    return { change, undo: { code: 'chat.removeFillers', params: {} } };
  },
});

let on = false;

/** Control Shift P: the stand-in chat on or off. Control Shift W: sample words for the open project's clips. */
export function listenForDevKeys(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (!e.ctrlKey || !e.shiftKey) return;
    if (e.code === 'KeyP') {
      e.preventDefault();
      on = !on;
      installChatBridge(on ? devChat : tauriChat);
      // The stand-in needs no key or code: chat runs while it is on, the AI status left as Rust said.
      useChatStandIn.setState({ on });
      toast.info(on ? 'Stand-in chat is on.' : 'Stand-in chat is off.');
    } else if (e.code === 'KeyW') {
      e.preventDefault();
      speech.devSample().then(
        (clips) => toast.info(clips === 1 ? 'Sample words for 1 clip.' : `Sample words for ${clips} clips.`),
        (error: unknown) => toast.error(errorMessage(error)),
      );
    }
  };
  window.addEventListener('keydown', onKey);
  return () => window.removeEventListener('keydown', onKey);
}
