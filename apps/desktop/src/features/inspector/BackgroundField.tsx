import { useLingui } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import type { Background } from '../../bindings/Background';
import type { BackgroundKind } from '../../bindings/BackgroundKind';
import type { MediaSource } from '../../bindings/MediaSource';
import type { PackItem } from '../../bindings/PackItem';
import { bridge } from '../../bridge/commands';
import { library } from '../../bridge/library';
import type { Label } from '../../lib/labels';
import { errorMessage } from '../../lib/errors';
import { queueCommand } from '../../state/commit';
import { commitLatest } from '../../state/latest';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';
import { backgroundOps, kindPatch } from './backgroundOps';
import { Field, Select, Swatches } from './fields';
import { textSwatches } from './TextInspector';
import { ToolSlider } from './ToolSlider';

/** The model's widest padding: 40 % of the frame's shorter side on every edge. */
const MAX_PADDING = 0.4;
const percent = (v: number) => `${Math.round(v * 100)}%`;

/** The library's pictures (its stickers), read once the Picture field shows; none until they are. */
function useLibraryPictures(wanted: boolean): PackItem[] {
  const [pictures, setPictures] = useState<PackItem[]>([]);
  useEffect(() => {
    if (!wanted) return;
    let gone = false;
    library.pack().then(
      (items) => {
        if (!gone) setPictures(items.filter((item) => item.category === 'sticker'));
      },
      // No library here: the project's pictures and a file are still there.
      () => {},
    );
    return () => {
      gone = true;
    };
  }, [wanted]);
  return pictures;
}

/** `source` as the background picture, one undo step at its turn; says why when it can't be. */
function pictureBehind(source: MediaSource) {
  void queueCommand(() => library.setBackground(source));
}

/** A picture chosen on this Mac as the background; nothing when the picker is closed without one. */
async function fileBehind() {
  try {
    const chosen = await bridge.choosePicture();
    const picture = chosen.assets.find((asset) => asset.kind === 'image');
    if (picture) pictureBehind({ type: 'file', asset: picture });
  } catch (error) {
    toast.error(errorMessage(error));
  }
}

/** The Picture field's choice for a file chosen on this Mac. */
const FROM_FILE = 'file';

/**
 * What shows behind the video where it leaves the frame uncovered (under the clip card's Fit): Background
 * (Blurred video, the clip's own picture blurred and dimmed behind it, the usual look for wide video in a vertical
 * frame; Black; Color; Gradient; Picture), its colours or picture, and Padding (an inset round the video on every edge,
 * 0 to 40 % of the frame's shorter side). A picture is one of the project's images, one of the library's, or a file
 * chosen on this Mac (which comes into the project with it); a project without images shows the Picture field until
 * one is chosen. It is the project's one background, behind every clip, and a line under it says so. Each change is
 * one undo step built at its turn on the background as the project has it then (`backgroundOps`); a padding drag is
 * dry-run in the preview and commits once on release.
 */
export function BackgroundField() {
  const { t } = useLingui();
  const background = useProject((s) => s.snapshot?.project.background);
  const assets = useProject((s) => s.snapshot?.project.assets);
  // Picture picked in a project without images: its field shows until a picture is chosen or another kind is.
  const [choosing, setChoosing] = useState(false);
  const showPicture = background?.kind === 'image' || choosing;
  const libraryPictures = useLibraryPictures(showPicture);
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
  kinds.push({
    value: 'image',
    label: t({ id: 'background.picture', message: 'Picture', comment: 'Background: a picture' }),
  });
  const pickKind = (kind: BackgroundKind) => {
    // No picture to show yet: the Picture field asks for one first.
    if (kind === 'image' && !firstImage && background.kind !== 'image') return setChoosing(true);
    setChoosing(false);
    set('background.kind.undo', (now) => kindPatch(now, kind, firstImage));
  };
  const pickPicture = (choice: string) => {
    setChoosing(false);
    if (choice === FROM_FILE) return void fileBehind();
    if (choice.startsWith('pack:')) return pictureBehind({ type: 'pack', id: choice.slice('pack:'.length) });
    const assetId = choice.slice('asset:'.length);
    set('background.picture.undo', () => ({ kind: 'image', assetId }));
  };
  const pictureChoices = [
    ...images.map((a) => ({ value: `asset:${a.id}`, label: a.name })),
    ...libraryPictures.map((item) => ({ value: `pack:${item.id}`, label: item.name })),
    {
      value: FROM_FILE,
      label: t({ id: 'background.picture.file', message: 'Choose a file…', comment: 'Picks a picture on this Mac' }),
    },
  ];
  const label = t({ id: 'clip.background', message: 'Background', comment: 'What shows behind the video' });
  const padding = t({ id: 'background.padding', message: 'Padding', comment: 'Space round the video' });
  const swatches = textSwatches(t);
  return (
    <>
      <Field label={label}>
        <Select label={label} value={choosing ? 'image' : background.kind} choices={kinds} onPick={pickKind} />
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
      {showPicture && (
        <Field label={t({ id: 'background.picture.short', message: 'Picture', comment: 'The background picture field' })}>
          <Select
            label={t({ id: 'background.picture.named', message: 'Background picture' })}
            value={background.kind === 'image' && background.assetId ? `asset:${background.assetId}` : ''}
            choices={pictureChoices}
            placeholder={t({
              id: 'background.picture.choose',
              message: 'Choose a picture',
              comment: 'The background picture field before a picture is chosen',
            })}
            onPick={pickPicture}
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
