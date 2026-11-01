import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invoke(cmd, args),
  Channel: class<T> {
    onmessage: (message: T) => void = () => {};
  },
}));

import { engine, type EngineEvent, type EngineInfo } from './engine';

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
    await expect(engine.attach((e) => seen.push(e))).resolves.toEqual(info);
    expect(invoke).toHaveBeenCalledTimes(1);
    const [cmd, args] = invoke.mock.calls[0];
    expect(cmd).toBe('engine_attach');
    const events = (args as { events: { onmessage: (e: EngineEvent) => void } }).events;
    events.onmessage({ type: 'ready', session: 3 });
    expect(seen).toEqual([{ type: 'ready', session: 3 }]);
  });

  it('sends the view, the native rect and seeks with the names Rust expects', async () => {
    invoke.mockResolvedValue(undefined);
    await engine.setView({ format: 'portrait', width: 540, height: 960 });
    await engine.setRect({ x: 1, y: 2, width: 3, height: 4, scale: 2, backdrop: null });
    await engine.setRect(null);
    await engine.seek(1_500_000, 7);
    expect(invoke.mock.calls).toEqual([
      ['engine_set_view', { view: { format: 'portrait', width: 540, height: 960 } }],
      ['engine_view', { rect: { x: 1, y: 2, width: 3, height: 4, scale: 2, backdrop: null } }],
      ['engine_view', { rect: null }],
      ['engine_seek', { atUs: 1_500_000, seq: 7 }],
    ]);
  });

  it('plays from a time at a rate as a numbered run, and pauses to the time it stopped at', async () => {
    invoke.mockResolvedValueOnce(undefined).mockResolvedValueOnce(2_000_000);
    await engine.play(250_000, 1, 3);
    await expect(engine.pause()).resolves.toBe(2_000_000);
    expect(invoke.mock.calls).toEqual([
      ['engine_play', { fromUs: 250_000, rate: 1, seq: 3 }],
      ['engine_pause', undefined],
    ]);
  });

  it('tells the engine when the window is hidden and shown', async () => {
    invoke.mockResolvedValue(undefined);
    await engine.setVisible(false);
    await engine.setVisible(true);
    expect(invoke.mock.calls).toEqual([
      ['engine_set_visible', { visible: false }],
      ['engine_set_visible', { visible: true }],
    ]);
  });

  it('sets the listening volume', async () => {
    invoke.mockResolvedValue(undefined);
    await engine.setVolume(0.25);
    expect(invoke.mock.calls).toEqual([['engine_set_volume', { volume: 0.25 }]]);
  });

  it('dry-runs edits on a project revision and clears them', async () => {
    invoke.mockResolvedValue(undefined);
    await engine.preview('p', 4, [{ type: 'rename', name: 'X' }]);
    await engine.previewClear();
    expect(invoke.mock.calls).toEqual([
      ['engine_preview', { projectId: 'p', baseRevision: 4, ops: [{ type: 'rename', name: 'X' }] }],
      ['engine_preview_clear', undefined],
    ]);
  });
});
