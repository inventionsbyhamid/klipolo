import { useLingui } from '@lingui/react/macro';
import { useMemo, useState } from 'react';
import type { Project } from '../../bindings/Project';
import type { Track } from '../../bindings/Track';
import { useUi } from '../../state/ui';
import { AddTrackRow } from './lanes/AddTrackRow';
import { AudioLane } from './lanes/AudioLane';
import { audioLines } from './lanes/audioRole';
import { LaneHeader } from './lanes/LaneHeader';
import { LaneRow } from './lanes/LaneRow';
import { LayersLane, layersLanePx } from './lanes/LayersLane';
import { MainLane } from './lanes/MainLane';
import { NewTrackSlot } from './lanes/NewTrackZone';
import { SourceLanes } from './lanes/SourceLanes';
import type { LineView } from './lanes/TrackLane';
import { TrackRow } from './lanes/TrackRow';
import { AUDIO_TRACK_PX, CLIP_INNER_PX, MAIN_LANE_PX, TRACK_ROW_PX } from './layout';
import { laneTracks, type LaneId } from './model/laneOps';
import { createZoomRest } from './media/zoomRest';
import { ViewportContext, ZoomRestContext } from './media/ViewportContext';
import { MarkerFlags } from './MarkerFlags';
import { ProposalOverlay } from './ProposalOverlay';
import { TrackHeader } from './TrackHeader';
import { useLaneSizes } from './useLaneSizes';
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
 * three rows), Video (the clips, taking the spare height, with a recording's marker flags), Sounds and Music; and
 * between Video and Sounds, one lane per kind of sound the takes recorded (`SourceLanes`), only while a take has it.
 * Overlays, Sound effects and Music have their header while a track is on them (`LaneHeader`: options, Hide or Mute).
 * Empty lanes are a faint strip. Show all tracks lists every track in a row of its own instead of the Overlays, Sound
 * effects and Music lanes: the picture tracks above Video (the top one first), the audio tracks under the recorded
 * sounds, each with its header (`TrackHeader`), then the new-track zone a dragged layer can go to and the buttons that
 * add a track. It gives the blocks' media the window it draws in and this timeline's zoom gestures to wait out
 * (`media/BlockMedia`).
 */
export function TimelineLanes({ project, viewport, view, width, clipPx = CLIP_INNER_PX, onSeek }: TimelineLanesProps) {
  const { t } = useLingui();
  const expanded = useUi((s) => s.timelineExpanded);
  const { main, tracks } = project.timeline;
  const { assets, recordings } = project;
  // One object per asset and recording lists, which the source lanes' kinds are kept by.
  const sources = useMemo(() => ({ assets, recordings }), [assets, recordings]);
  const [zoomRest] = useState(() => createZoomRest());
  // A new value only when the window steps (or on a zoom), so the blocks' media re-renders only then.
  const { fromUs, toUs, widthPx, scrollPx } = viewport;
  const drawWindow = useMemo(() => ({ fromUs, toUs, widthPx, scrollPx }), [fromUs, toUs, widthPx, scrollPx]);
  // A waiting proposal's removed layers, outlined on their lanes: the lanes' own window and zoom.
  const ghostWindow = { pps: view.pps, fromUs: view.fromUs, toUs: view.toUs };
  const sounds = audioLines(tracks);
  const sizes = useLaneSizes();
  const names = {
    layers: t({
      id: 'timeline.lane.overlays',
      message: 'Overlays',
      comment: 'The lane of titles, pictures and video layers',
    }),
    sounds: t({ id: 'timeline.lane.sounds', message: 'Sound effects' }),
    music: t({ id: 'timeline.lane.music', message: 'Music' }),
  };
  /** A lane with a track of its own has its header: its options, and Hide or Mute. */
  const header = (lane: LaneId, icon: 'layers' | 'pulse' | 'music') =>
    laneTracks(tracks, lane).length > 0 ? (
      <LaneHeader lane={lane} name={names[lane]} icon={icon} tracks={tracks} />
    ) : undefined;
  /** A track's own row, while every track shows. */
  const trackRow = (track: Track) => {
    const rowPx = track.kind === 'audio' ? AUDIO_TRACK_PX : TRACK_ROW_PX;
    return (
      <LaneRow
        key={track.id}
        lane={track.id}
        track={track}
        name={track.name}
        header={<TrackHeader track={track} />}
        empty={track.items.length === 0}
        height={rowPx}
        width={width}
        onSeek={onSeek}
      >
        <TrackRow track={track} assets={assets} recordings={recordings} rowPx={rowPx} {...view} />
        <ProposalOverlay lane="track" track={track} rowPx={rowPx} {...ghostWindow} />
      </LaneRow>
    );
  };
  // The four lanes' own: Overlays above Video, Sound effects and Music under the recorded sounds.
  const layersLane = (
    <LaneRow
      lane="layers"
      icon="layers"
      name={names.layers}
      header={header('layers', 'layers')}
      empty={!tracks.some((track) => track.kind !== 'audio' && track.items.length > 0)}
      height={layersLanePx(tracks)}
      width={width}
      onSeek={onSeek}
    >
      <LayersLane tracks={tracks} assets={assets} recordings={recordings} {...view} />
      <ProposalOverlay lane="layers" tracks={tracks} {...ghostWindow} />
    </LaneRow>
  );
  const soundLanes = (['sounds', 'music'] as const).map((role) => (
    <LaneRow
      key={role}
      lane={role}
      icon={role === 'music' ? 'music' : 'pulse'}
      name={names[role]}
      header={header(role, role === 'music' ? 'music' : 'pulse')}
      empty={sounds[role].items.length === 0}
      height={sizes[role]}
      width={width}
      onSeek={onSeek}
    >
      <AudioLane role={role} rowPx={sizes[role]} tracks={tracks} assets={assets} recordings={recordings} {...view} />
      <ProposalOverlay lane={role} rowPx={sizes[role]} tracks={tracks} {...ghostWindow} />
    </LaneRow>
  ));
  return (
    <ViewportContext value={drawWindow}>
      <ZoomRestContext value={zoomRest}>
        {expanded ? (
          <>
            {/* A dragged overlay or zoom layer's new-track zone, then the picture tracks, top first, as they draw. */}
            <NewTrackSlot at="top" />
            {tracks
              .filter((track) => track.kind !== 'audio')
              .reverse()
              .map(trackRow)}
          </>
        ) : (
          layersLane
        )}
        <LaneRow
          lane="main"
          icon="film"
          name={t({ id: 'timeline.lane.video', message: 'Video' })}
          empty={main.length === 0}
          // Among every track's row it keeps its own height; with the four lanes it takes the spare height.
          grow={!expanded}
          height={MAIN_LANE_PX}
          width={width}
          onSeek={onSeek}
        >
          <MainLane
            main={main}
            assets={assets}
            recordings={recordings}
            fps={project.settings.fps}
            innerPx={clipPx}
            {...view}
          />
          <MarkerFlags recordings={recordings} main={main} fps={project.settings.fps} {...ghostWindow} />
        </LaneRow>
        {/* Each kind of recorded sound's own lane, under the video it was recorded with. */}
        <SourceLanes
          clips={main}
          sources={sources}
          tracks={tracks}
          view={ghostWindow}
          rowPx={sizes.source}
          width={width}
          onSeek={onSeek}
        />
        {expanded ? (
          <>
            {tracks.filter((track) => track.kind === 'audio').map(trackRow)}
            {/* A dragged sound's new-track zone, then the buttons that add a track. */}
            <NewTrackSlot at="bottom" />
            <AddTrackRow />
          </>
        ) : (
          soundLanes
        )}
      </ZoomRestContext>
    </ViewportContext>
  );
}
