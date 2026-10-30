import {
  useLayoutEffect,
  useRef,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type RefObject,
} from 'react';
import { clipDurationUs, clipStarts, sourceLabel } from '../lib/timeline';
import { stepFrames } from '../lib/time';
import { formatTime } from '../player/time';
import { clockAnchor, clockUs, onClock } from '../state/playClock';
import { usePlayback } from '../state/playback';
import { useProject } from '../state/project';
import { useDurationUs } from '../state/transport';
import { Icon } from './Icon';

/** Seconds between labelled ticks: the first step that gives at most 8 labels. */
const STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];

/** `m:ss` (or `h:mm:ss`) for a ruler label. */
function label(seconds: number): string {
  const s = seconds % 60;
  const m = Math.floor(seconds / 60) % 60;
  const h = Math.floor(seconds / 3600);
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** The ruler's labels and tick spacing for a timeline `durationUs` long (40 s when empty). */
export function ruler(durationUs: number) {
  const seconds = durationUs > 0 ? durationUs / 1e6 : 40;
  const step = STEPS.find((s) => seconds / s <= 8) ?? STEPS[STEPS.length - 1];
  const share = (s: number) => `${(s / seconds) * 100}%`;
  const labels = [];
  for (let t = 0; t < seconds; t += step) labels.push({ x: share(t), text: label(t) });
  return { major: share(step), minor: share(step / 5), labels };
}

const percent = (us: number, durationUs: number) =>
  `${(Math.min(Math.max(us, 0), durationUs) / durationUs) * 100}%`;

/**
 * Moves the playhead and the slider's value outside React: paused, it sits where the store says; playing, it runs on
 * the play clock with a Web Animation to the end (re-aimed only when the clock re-anchors), never a render per frame.
 */
function usePlayheadMotion(
  slider: RefObject<HTMLDivElement | null>,
  head: RefObject<HTMLSpanElement | null>,
  durationUs: number,
) {
  useLayoutEffect(() => {
    const bar = slider.current;
    const el = head.current;
    if (!bar || !el || durationUs <= 0) return;
    let motion: Animation | null = null;
    const follow = () => {
      motion?.cancel();
      motion = null;
      const { playing, playheadUs } = usePlayback.getState();
      const anchor = clockAnchor();
      const at = playing && anchor.running ? clockUs() : playheadUs;
      bar.setAttribute('aria-valuenow', String(Math.round(Math.min(at, durationUs))));
      bar.setAttribute('aria-valuetext', formatTime(Math.min(at, durationUs)));
      if (!playing || !anchor.running || typeof el.animate !== 'function') {
        el.style.left = percent(at, durationUs);
        return;
      }
      const end = anchor.rate > 0 ? durationUs : 0;
      const ms = Math.abs(end - at) / 1000 / Math.abs(anchor.rate);
      el.style.left = percent(end, durationUs);
      motion = el.animate([{ left: percent(at, durationUs) }, { left: percent(end, durationUs) }], {
        duration: ms,
        easing: 'linear',
      });
    };
    follow();
    const offClock = onClock(follow);
    const offStore = usePlayback.subscribe((s, prev) => {
      if (s.playing !== prev.playing || (!s.playing && s.playheadUs !== prev.playheadUs)) follow();
    });
    return () => {
      motion?.cancel();
      offClock();
      offStore();
    };
  }, [slider, head, durationUs]);
}

/**
 * The timeline area (overview, ruler, overlay, video, sound effect and music lanes): the
 * main track's clips in order, the playhead, and seeking by click or drag. Editing lands with the full timeline.
 */
export function TimelinePanel() {
  const project = useProject((s) => s.snapshot?.project ?? null);
  const duration = useDurationUs();
  const r = ruler(duration);
  const main = project?.timeline.main ?? [];
  const starts = clipStarts(main);
  const ready = duration > 0;
  const slider = useRef<HTMLDivElement>(null);
  const head = useRef<HTMLSpanElement>(null);
  const dragging = useRef(false);
  usePlayheadMotion(slider, head, duration);

  const toUs = (clientX: number) => {
    const box = slider.current!.getBoundingClientRect();
    return Math.min(Math.max((clientX - box.left) / box.width, 0), 1) * duration;
  };
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (!ready || e.button !== 0) return;
    dragging.current = true;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    usePlayback.getState().seek(toUs(e.clientX));
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (dragging.current) usePlayback.getState().seek(toUs(e.clientX));
  };
  const onPointerUp = () => {
    dragging.current = false;
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!ready || !project || (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight')) return;
    e.preventDefault();
    e.stopPropagation();
    const { playing, playheadUs, seek } = usePlayback.getState();
    const at = playing ? clockUs() : playheadUs;
    const back = e.key === 'ArrowLeft';
    // A frame at a time; with Shift, a second.
    seek(
      e.shiftKey
        ? Math.min(Math.max(at + (back ? -1_000_000 : 1_000_000), 0), duration)
        : stepFrames(at, back ? -1 : 1, project.settings.fps, duration),
    );
  };
  return (
    <section className="panel timeline" aria-label="Timeline">
      <div className="tl">
        <span className="tl-ico r-overlay" title="Text and images">
          <Icon name="layers" />
        </span>
        <span className="tl-ico r-video" title="Video">
          <Icon name="film" />
        </span>
        <span className="tl-ico r-sfx" title="Sound effects">
          <Icon name="pulse" />
        </span>
        <span className="tl-ico r-music" title="Music">
          <Icon name="music" />
        </span>
        <div className="tl-overview">{main.length > 0 && <i className="ov-seg" />}</div>
        <div className="tl-ruler" style={{ '--minor': r.minor, '--major': r.major } as CSSProperties}>
          {r.labels.map((l) => (
            <span key={l.text} style={{ '--x': l.x } as CSSProperties}>
              {l.text}
            </span>
          ))}
        </div>
        <div className="lane r-overlay" />
        <div className="lane r-video">
          {project &&
            main.map((clip, i) => (
              <div
                key={clip.id}
                className="clip-block"
                data-testid="clip-block"
                style={{
                  left: percent(starts[i], duration),
                  width: `${(clipDurationUs(clip) / duration) * 100}%`,
                  right: 'auto',
                }}
              >
                {sourceLabel(project, clip.source)}
              </div>
            ))}
        </div>
        <div className="lane r-sfx" />
        <div className="lane r-music" />
        {main.length === 0 && <p className="tl-empty">Clips line up here in the order you add them.</p>}
        <div
          ref={slider}
          className="tl-marks"
          role="slider"
          tabIndex={ready ? 0 : -1}
          aria-label="Playhead"
          aria-disabled={!ready}
          aria-valuemin={0}
          aria-valuemax={duration}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={onPointerUp}
          onKeyDown={onKeyDown}
        >
          {ready && <span ref={head} className="playhead" />}
        </div>
      </div>
    </section>
  );
}
