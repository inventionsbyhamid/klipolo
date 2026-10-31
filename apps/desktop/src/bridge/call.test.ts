import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const { call } = await import('./call');

describe('call', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('words a coded error from Rust in the language shown, and passes a sentence through', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'engine.play.rateUnavailable', params: {} });
    await expect(call('engine_play', { fromUs: 0, rate: 3 })).rejects.toThrow(
      "That playback speed isn't available.",
    );
    // A code without a case of its own is worded from Rust's tables: the model's, with its values.
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'model.project.name', params: {} });
    await expect(call('project_apply', {})).rejects.toThrow('The project name must be 1 to 200 characters.');
    vi.mocked(invoke).mockRejectedValueOnce("The preview isn't ready yet.");
    await expect(call('engine_play', { fromUs: 0 })).rejects.toThrow("The preview isn't ready yet.");
  });

  it('words the damaged-sound notice with the file and where its sound stops', async () => {
    const { codedMessage } = await import('./call');
    expect(
      codedMessage({ code: 'engine.audioDamaged', params: { file: 'interview.mov', atUs: 2_412_000_000 } }),
    ).toBe(
      "The sound of interview.mov can't be read after 40:12, so it's silent from there. The file may be damaged.",
    );
  });

  it('says plainly when the app has no such command, keeping what Tauri said as the cause', async () => {
    vi.mocked(invoke).mockRejectedValueOnce('Command chat_draw_picture not found');
    const error = (await call('chat_draw_picture', { id: 'p' }).catch((e: unknown) => e)) as Error;
    expect(error.message).toBe("Klipolo can't do that in this version. Restart Klipolo and try again.");
    expect(error.cause).toBe('Command chat_draw_picture not found');
  });
});
