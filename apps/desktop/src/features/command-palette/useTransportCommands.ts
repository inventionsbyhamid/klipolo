import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import { stepFrames } from '../../lib/time';
import { videoDurationUs } from '../../lib/timeline';
import { useLoop } from '../../state/loop';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useRange } from '../../state/range';
import { searchWords, type Command } from './command';

/**
 * The transport: play and pause, the J/L shuttle (backward and forward, again for double speed), Loop and frame steps.
 * The palette, the keys (Space, J/K/L, K held with J/L, ⌘L, ← and →) and the transport's buttons all run these. Frame
 * steps go by frame index on the engine's grid, so each shows the next frame exactly once. It re-renders on play, pause,
 * a speed change, Loop, a time range coming or going, or a new frame rate or length, never per frame: the playhead is
 * read when a command runs.
 */
export function useTransportCommands(): Command[] {
  const { t } = useLingui();
  const fps = useProject((s) => s.snapshot?.project.settings.fps ?? 30);
  const durationUs = useProject((s) => (s.snapshot ? videoDurationUs(s.snapshot.project.timeline) : 0));
  const playing = usePlayback((s) => s.playing);
  const rate = usePlayback((s) => s.rate);
  const looping = useLoop((s) => s.on);
  const ranged = useRange((s) => s.range !== null);
  return useMemo((): Command[] => {
    const hasVideo = durationUs > 0;
    /** One frame (⇧ one second) back or on, within the video's frames. */
    const stepFrame = (direction: 1 | -1, large = false) => {
      const { playheadUs, seek } = usePlayback.getState();
      seek(stepFrames(playheadUs, direction * (large ? Math.round(fps) : 1), fps, durationUs));
    };
    const space = t({ id: 'key.space', message: 'Space', comment: 'The space bar, as a keyboard shortcut' });
    const frameWords = searchWords(
      t({
        id: 'command.frame.keywords',
        message: 'step',
        comment: 'Comma-separated search words for frame steps',
      }),
    );

    return [
      {
        id: 'play',
        action: 'playback',
        group: 'playback',
        label: playing
          ? t({ id: 'playback.pause', message: 'Pause' })
          : t({ id: 'playback.play', message: 'Play' }),
        shortcut: playing ? [space, 'K'] : space,
        enabled: hasVideo,
        run: () => usePlayback.getState().toggle(),
      },
      {
        id: 'pause',
        action: 'playback',
        group: 'playback',
        label: t({ id: 'playback.pause', message: 'Pause' }),
        shortcut: 'K',
        enabled: playing,
        paletteHidden: true,
        run: () => usePlayback.getState().pause(),
      },
      {
        id: 'play-backward',
        action: 'shuttle',
        group: 'playback',
        label:
          playing && rate < 0
            ? t({
                id: 'command.playBackwardFaster',
                message: 'Play backward faster',
                comment: 'J again: double speed',
              })
            : t({ id: 'command.playBackward', message: 'Play backward' }),
        shortcut: 'J',
        keywords: searchWords(
          t({
            id: 'command.playBackward.keywords',
            message: 'reverse, rewind, shuttle',
            comment: 'Comma-separated search words for Play backward',
          }),
        ),
        // At double speed there is no faster.
        enabled: hasVideo && !(playing && rate === -2),
        run: () => usePlayback.getState().shuttle(-1),
      },
      {
        id: 'play-forward',
        action: 'shuttle',
        group: 'playback',
        label:
          playing && rate > 0
            ? t({
                id: 'command.playFaster',
                message: 'Play faster',
                comment: 'L while playing: double speed',
              })
            : t({ id: 'command.playForward', message: 'Play forward' }),
        shortcut: 'L',
        keywords: searchWords(
          t({
            id: 'command.playForward.keywords',
            message: 'fast forward, double speed, shuttle',
            comment: 'Comma-separated search words for Play forward and Play faster',
          }),
        ),
        enabled: hasVideo && !(playing && rate === 2),
        run: () => usePlayback.getState().shuttle(1),
      },
      {
        id: 'loop',
        action: 'loop',
        group: 'playback',
        label: looping
          ? ranged
            ? t({ id: 'command.loop.stopSelection', message: 'Stop looping the selection' })
            : t({ id: 'command.loop.stopVideo', message: 'Stop looping the video' })
          : ranged
            ? t({ id: 'command.loop.selection', message: 'Loop the selection' })
            : t({ id: 'command.loop.video', message: 'Loop the video' }),
        shortcut: '⌘L',
        keywords: searchWords(
          t({
            id: 'command.loop.keywords',
            message: 'repeat, again, cycle',
            comment: 'Comma-separated search words for Loop',
          }),
        ),
        enabled: hasVideo,
        run: () => useLoop.getState().toggle(),
      },
      {
        id: 'frame-back',
        action: 'goTo',
        group: 'playback',
        label: t({ id: 'playback.previousFrame', message: 'Previous frame' }),
        shortcut: '←',
        keywords: frameWords,
        enabled: hasVideo,
        run: (options) => stepFrame(-1, options?.large),
      },
      {
        id: 'frame-forward',
        action: 'goTo',
        group: 'playback',
        label: t({ id: 'playback.nextFrame', message: 'Next frame' }),
        shortcut: '→',
        keywords: frameWords,
        enabled: hasVideo,
        run: (options) => stepFrame(1, options?.large),
      },
    ];
  }, [t, fps, durationUs, playing, rate, looping, ranged]);
}
