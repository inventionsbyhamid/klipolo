import { useEffect } from 'react';
import type { EngineEvent } from '../../bindings/EngineEvent';
import { codedMessage } from '../../bridge/call';
import { engine } from '../../bridge/engine';
import { perfAt } from '../../lib/clocks';
import { errorMessage } from '../../lib/errors';
import { isCurrentFrame, useEngine } from '../../state/engine';
import { useMedia } from '../../state/media';
import { sendSeek, usePlayback } from '../../state/playback';
import { toast } from '../../state/toasts';
import { resendEditing } from './textEdit';

/** What the page does with an event from the preview engine (exported for its tests). */
export function onEngineEvent(event: EngineEvent) {
  switch (event.type) {
    case 'ready':
      useEngine.getState().setSession(event.session);
      // A new session starts at 0 in the engine; show where the playhead actually is.
      sendSeek(usePlayback.getState().playheadUs);
      // A title typed on the canvas meanwhile is left out of the new session's frames too.
      resendEditing();
      break;
    case 'warning':
      toast.error(event.message);
      break;
    case 'error':
      useEngine.getState().setStatus('unavailable', event.message);
      break;
    case 'state':
      usePlayback.getState().onState(event);
      break;
    case 'shown': {
      // The native layer put a frame on screen (stills, and a few times a second while playing): the clock, the
      // playhead and the "live" state follow it (socket frames carry the same in their trailer).
      if (!isCurrentFrame(event)) break;
      usePlayback.getState().onFrame({
        timeUs: event.atUs,
        seq: event.seq,
        playback: event.playback,
        atMs: perfAt(event.shownMs),
      });
      const now = useEngine.getState();
      if (now.status !== 'live') now.setStatus('live');
      break;
    }
    case 'presenter':
      // The native layer failed: frames come over the socket now (the canvas takes over).
      useEngine.getState().setPresenter(event.presenter);
      break;
    case 'media':
      // A file's waveform or thumbnails are ready: what draws them reloads. Its screen activity isn't drawn.
      if (event.kind !== 'activity') useMedia.getState().bump(event.path);
      break;
    case 'notice': {
      // Something the person should know once, e.g. a file whose sound is damaged part-way.
      const message = codedMessage(event.notice);
      if (message) toast.error(message);
      break;
    }
    case 'waiting':
      // A disk silent for a while: a quiet note under the preview while it waits, gone once it answers.
      useEngine.getState().setWaiting(event.notice);
      break;
  }
}

/** Attaches the engine while the editor is open and tells it when the window is hidden. */
export function useEngineSession() {
  const attempt = useEngine((s) => s.attempt);
  useEffect(() => {
    let disposed = false;
    const store = useEngine.getState();
    store.setStatus('starting');
    const onVisibility = () =>
      void engine.setVisible(document.visibilityState === 'visible').catch(() => undefined);
    engine.attach(onEngineEvent).then(
      (info) => {
        if (disposed) return;
        store.setInfo(info);
        if (info.port === 0) store.setStatus('unavailable');
        onVisibility(); // the engine only listens once attached
      },
      (error) => !disposed && store.setStatus('unavailable', errorMessage(error)),
    );
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      disposed = true;
      document.removeEventListener('visibilitychange', onVisibility);
      // Leaving the editor stops playback (and its sound) at once.
      usePlayback.getState().pause();
      useEngine.getState().reset();
    };
  }, [attempt]);
}
