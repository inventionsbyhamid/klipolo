import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { call } from './call';

/** What was said in the clips, as the engine's feed has it (analysis puts it there). */
export const speech = {
  /**
   * The feed's speech changed: the captions were made again. `files`: the files whose words were heard, timed again or
   * taken away, by asset id.
   */
  onChanged: (handler: (files: string[]) => void): Promise<UnlistenFn> =>
    listen<string[] | null>('speech-changed', (event) => handler(event.payload ?? [])),
  /** Development builds: a sample transcript for every clip of the open project. How many clips got one. */
  devSample: () => call<number>('dev_fixture_speech'),
};
