import { useLingui } from '@lingui/react/macro';
import { useMemo, useState } from 'react';
import type { Project } from '../../bindings/Project';
import { AudioLane } from './lanes/AudioLane';
import { audioLines } from './lanes/audioRole';
import { LaneRow } from './lanes/LaneRow';
import { LayersLane, layersLanePx } from './lanes/LayersLane';
import { MainLane } from './lanes/MainLane';
import type { LineView } from './lanes/TrackLane';
import { CLIP_INNER_PX, MUSIC_LANE_PX, SOUNDS_LANE_PX } from './layout';
import { createZoomRest } from './media/zoomRest';
import { ViewportContext, ZoomRestContext } from './media/ViewportContext';
import type { Viewport } from './useViewport';

interface TimelineLanesProps {
  project: Project;
  /** The lanes' window (`useViewport`), which the blocks' media draws its part of. */
  viewport: Viewport;
  /** What every lane draws with: the zoom and window, the selection and focus, the blocks' handlers. */
  view: Omit<LineView, 'sources'>;
  width: number;
  /** A clip's inside (px): the Video lane takes the timeline's spare height. */
  clipPx?: number;
  /** A click on empty lane: seek there. */
  onSeek: (px: number) => void;
}

/**
 * The timeline's four lanes under the ruler, always shown: Overlays (text and images, packed in up to
 * three rows), Video (the clips, taking the spare height), Sounds and Music. Empty lanes are a faint strip. It gives
 * the blocks' media the window it draws in and this timeline's zoom gestures to wait out (`media/BlockMedia`).
 */
export function TimelineLanes({ project, viewport, view, width, clipPx = CLIP_INNER_PX, onSeek }: TimelineLanesProps) {
  const { t } = useLingui();
  const { main, tracks } = project.timeline;
  const { assets, recordings } = project;
  const [zoomRest] = useState(() => createZoomRest());
  // A new value only when the window steps (or on a zoom), so the blocks' media re-renders only then.
  const { fromUs, toUs, widthPx, scrollPx } = viewport;
  const drawWindow = useMemo(() => ({ fromUs, toUs, widthPx, scrollPx }), [fromUs, toUs, widthPx, scrollPx]);
  const sounds = audioLines(tracks);
  return (
    <ViewportContext value={drawWindow}>
      <ZoomRestContext value={zoomRest}>
        <LaneRow
          lane="layers"
          icon="layers"
          name={t({ id: 'timeline.lane.overlays', message: 'Text and images' })}
          empty={!tracks.some((track) => track.kind !== 'audio' && track.items.length > 0)}
          height={layersLanePx(tracks)}
          width={width}
          onSeek={onSeek}
        >
          <LayersLane tracks={tracks} assets={assets} recordings={recordings} {...view} />
        </LaneRow>
        <LaneRow
          lane="main"
          icon="film"
          name={t({ id: 'timeline.lane.video', message: 'Video' })}
          empty={main.length === 0}
          grow
          width={width}
          onSeek={onSeek}
        >
          <MainLane main={main} assets={assets} recordings={recordings} innerPx={clipPx} {...view} />
        </LaneRow>
        <LaneRow
          lane="sounds"
          icon="pulse"
          name={t({ id: 'timeline.lane.sounds', message: 'Sound effects' })}
          empty={sounds.sounds.items.length === 0}
          height={SOUNDS_LANE_PX}
          width={width}
          onSeek={onSeek}
        >
          <AudioLane role="sounds" tracks={tracks} assets={assets} recordings={recordings} {...view} />
        </LaneRow>
        <LaneRow
          lane="music"
          icon="music"
          name={t({ id: 'timeline.lane.music', message: 'Music' })}
          empty={sounds.music.items.length === 0}
          height={MUSIC_LANE_PX}
          width={width}
          onSeek={onSeek}
        >
          <AudioLane role="music" tracks={tracks} assets={assets} recordings={recordings} {...view} />
        </LaneRow>
      </ZoomRestContext>
    </ViewportContext>
  );
}
