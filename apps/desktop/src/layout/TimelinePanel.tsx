import type { CSSProperties } from 'react';
import { Scrub } from '../player/Scrub';
import { useSession } from '../player/session';
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

/**
 * The timeline area (overview, ruler, overlay, video, sound effect and music lanes). For
 * now it shows the open video as one clip and moves the playhead; editing lands with the timeline.
 */
export function TimelinePanel() {
  const project = useSession((s) => s.project);
  const r = ruler(project?.durationUs ?? 0);
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
        <div className="tl-overview">{project && <i className="ov-seg" />}</div>
        <div className="tl-ruler" style={{ '--minor': r.minor, '--major': r.major } as CSSProperties}>
          {r.labels.map((l) => (
            <span key={l.text} style={{ '--x': l.x } as CSSProperties}>
              {l.text}
            </span>
          ))}
        </div>
        <div className="lane r-overlay" />
        <div className="lane r-video">{project && <div className="clip-block">{project.name}</div>}</div>
        <div className="lane r-sfx" />
        <div className="lane r-music" />
        {!project && <p className="tl-empty">Clips line up here in the order you add them.</p>}
        <Scrub />
      </div>
    </section>
  );
}
