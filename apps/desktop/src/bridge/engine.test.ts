import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
  Channel: class<T> {
    onmessage: (message: T) => void = () => {};
  },
}));

import {
  engineAttach,
  enginePause,
  enginePlay,
  engineSeek,
  engineSetView,
  engineSetVisible,
  engineView,
  projectSummary,
  type EngineEvent,
  type EngineInfo,
} from './engine';

const info: EngineInfo = {
  port: 4321,
  token: 't',
  maxPreviewPixels: 1920 * 1080,
  adapter: 'test',
  presenter: 'native',
  gpuClass: 'integrated',
};

beforeEach(() => invoke.mockReset());

describe('engine bridge', () => {
  it('attaches with an event channel and hands each event to the callback', async () => {
    invoke.mockResolvedValue(info);
    const seen: EngineEvent[] = [];
    await expect(engineAttach((e) => seen.push(e))).resolves.toEqual(info);
    expect(invoke).toHaveBeenCalledTimes(1);
    const [cmd, args] = invoke.mock.calls[0];
    expect(cmd).toBe('engine_attach');
    const events = (args as { events: { onmessage: (e: EngineEvent) => void } }).events;
    events.onmessage({ type: 'ready', session: 3 });
    expect(seen).toEqual([{ type: 'ready', session: 3 }]);
  });

  it('sends the view, the native rect and seeks with the names Rust expects', async () => {
    invoke.mockResolvedValue(undefined);
    await engineSetView({ format: 'portrait', width: 540, height: 960 });
    await engineView({ x: 1, y: 2, width: 3, height: 4, scale: 2, backdrop: null });
    await engineView(null);
    await engineSeek(1_500_000, 7);
    expect(invoke.mock.calls).toEqual([
      ['engine_set_view', { view: { format: 'portrait', width: 540, height: 960 } }],
      ['engine_view', { rect: { x: 1, y: 2, width: 3, height: 4, scale: 2, backdrop: null } }],
      ['engine_view', { rect: null }],
      ['engine_seek', { atUs: 1_500_000, seq: 7 }],
    ]);
  });

  it('plays and pauses with no arguments and resolves the time in microseconds', async () => {
    invoke.mockResolvedValueOnce(250_000).mockResolvedValueOnce(2_000_000);
    await expect(enginePlay()).resolves.toBe(250_000);
    await expect(enginePause()).resolves.toBe(2_000_000);
    expect(invoke.mock.calls).toEqual([
      ['engine_play', undefined],
      ['engine_pause', undefined],
    ]);
  });

  it('tells the engine when the window is hidden and shown', async () => {
    invoke.mockResolvedValue(undefined);
    await engineSetVisible(false);
    await engineSetVisible(true);
    expect(invoke.mock.calls).toEqual([
      ['engine_set_visible', { visible: false }],
      ['engine_set_visible', { visible: true }],
    ]);
  });

  it('reads the open project summary', async () => {
    invoke.mockResolvedValue({ name: 'clip.mp4', durationUs: 10_000_000 });
    await expect(projectSummary()).resolves.toEqual({ name: 'clip.mp4', durationUs: 10_000_000 });
    expect(invoke).toHaveBeenCalledWith('project_summary', undefined);
  });
});
