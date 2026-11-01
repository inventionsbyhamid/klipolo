import { useLingui } from '@lingui/react/macro';
import type { Background } from '../../bindings/Background';
import type { BackgroundKind } from '../../bindings/BackgroundKind';
import type { Label } from '../../lib/labels';
import { commitLatest } from '../../state/latest';
import { useProject } from '../../state/project';
import { backgroundOps, kindPatch } from './backgroundOps';
import { Field, Select, Swatches } from './fields';
import { textSwatches } from './TextInspector';
import { ToolSlider } from './ToolSlider';

/** The model's widest padding: 40 % of the frame's shorter side on every edge. */
const MAX_PADDING = 0.4;
const percent = (v: number) => `${Math.round(v * 100)}%`;

/**
 * What shows behind the video where it leaves the frame uncovered (under the clip card's Fit): Background
 * (Blurred video, the clip's own picture blurred and dimmed behind it, the usual look for wide video in a vertical
 * frame; Black; Color; Gradient; Picture, one of the project's images, offered while it has one), its colours or
 * picture, and Padding (an inset round the video on every edge, 0 to 40 % of the frame's shorter side). It is the
 * project's one background, behind every clip, and a line under it says so. Each change is one undo step built at its
 * turn on the background as the project has it then (`backgroundOps`); a padding drag is dry-run in the preview and
 * commits once on release.
 */
export function BackgroundField() {
  const { t } = useLingui();
  const background = useProject((s) => s.snapshot?.project.background);
  const assets = useProject((s) => s.snapshot?.project.assets);
  if (!background) return null;
  const images = (assets ?? []).filter((a) => a.kind === 'image');
  const firstImage = images[0]?.id ?? null;
  const set = (label: Label, patch: (now: Background) => Partial<Background>) =>
    void commitLatest(label, () => backgroundOps(patch));
  const kinds: { value: BackgroundKind; label: string }[] = [
    {
      value: 'blur',
      label: t({
        id: 'background.blur',
        message: 'Blurred video',
        comment: 'Background behind a video: the video itself, blurred',
      }),
    },
    { value: 'none', label: t({ id: 'background.black', message: 'Black', comment: 'Background: plain black' }) },
    { value: 'solid', label: t({ id: 'background.color', message: 'Color', comment: 'Background: one color' }) },
    {
      value: 'gradient',
      label: t({ id: 'background.gradient', message: 'Gradient', comment: 'Background: two colors' }),
    },
  ];
  if (images.length > 0 || background.kind === 'image')
    kinds.push({
      value: 'image',
      label: t({ id: 'background.picture', message: 'Picture', comment: 'Background: one of the project’s images' }),
    });
  const label = t({ id: 'clip.background', message: 'Background', comment: 'What shows behind the video' });
  const padding = t({ id: 'background.padding', message: 'Padding', comment: 'Space round the video' });
  const swatches = textSwatches(t);
  return (
    <>
      <Field label={label}>
        <Select
          label={label}
          value={background.kind}
          choices={kinds}
          onPick={(kind) => set('background.kind.undo', (now) => kindPatch(now, kind, firstImage))}
        />
      </Field>
      <p className="field-note">
        {t({
          id: 'background.everyClip',
          message: 'Behind every clip.',
          comment: 'The background is the project’s one background, shown behind every clip',
        })}
      </p>
      {background.kind === 'solid' && (
        <Field label={t({ id: 'background.color.short', message: 'Color', comment: 'The background color field' })}>
          <Swatches
            label={t({ id: 'background.color.named', message: 'Background color' })}
            value={background.color}
            swatches={swatches}
            onPick={(color) => set('background.color.undo', () => ({ color }))}
          />
        </Field>
      )}
      {background.kind === 'gradient' && (
        <>
          <Field label={t({ id: 'background.from', message: 'From', comment: 'A gradient’s first color' })}>
            <Swatches
              label={t({ id: 'background.from.named', message: 'Gradient start color' })}
              value={background.color}
              swatches={swatches}
              onPick={(color) => set('background.color.undo', () => ({ color }))}
            />
          </Field>
          <Field label={t({ id: 'background.to', message: 'To', comment: 'A gradient’s second color' })}>
            <Swatches
              label={t({ id: 'background.to.named', message: 'Gradient end color' })}
              value={background.colorEnd ?? background.color}
              swatches={swatches}
              onPick={(colorEnd) => set('background.color.undo', () => ({ colorEnd }))}
            />
          </Field>
        </>
      )}
      {background.kind === 'image' && background.assetId && (
        <Field label={t({ id: 'background.picture.short', message: 'Picture', comment: 'The background picture field' })}>
          <Select
            label={t({ id: 'background.picture.named', message: 'Background picture' })}
            value={background.assetId}
            choices={images.map((a) => ({ value: a.id, label: a.name }))}
            onPick={(assetId) => set('background.picture.undo', () => ({ assetId }))}
          />
        </Field>
      )}
      <Field label={padding}>
        <ToolSlider
          label={padding}
          commitLabel="background.padding.undo"
          value={background.padding}
          min={0}
          max={MAX_PADDING}
          step={0.01}
          format={percent}
          ops={(value) => backgroundOps({ padding: value })}
        />
      </Field>
    </>
  );
}
