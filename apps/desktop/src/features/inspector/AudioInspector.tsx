import { useLingui } from '@lingui/react/macro';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import { Segmented } from '../../design/Segmented';
import { useSpeech } from '../timeline/media/speech';
import { TimingField } from './common';
import { Field } from './fields';
import { commitLatest, contentOps } from './latest';
import { ToolSlider } from './ToolSlider';

export type AudioContent = Extract<ItemContent, { type: 'audio' }>;

/** Music's ducking when it is turned on (the model's `MUSIC_DUCK_DB`), and the deepest it goes. */
const DUCK_DB = -12;
const DEEPEST_DB = -30;
/** Fade lengths offered (µs), each kept to half the layer so the two always fit. */
const FADES = [0, 500_000, 1_000_000, 2_000_000];
const percent = (v: number) => `${Math.round(v * 100)}%`;

/**
 * The fade choices for a layer `durationUs` long: None to 2 s, each at most half of it, without repeats, and the fade
 * it has (`currentUs`) when that's another length (chat's 0.3 s), so the control always shows what is set.
 */
export function fadeChoices(durationUs: number, currentUs: number): number[] {
  const half = Math.floor(durationUs / 2);
  const choices = new Set(FADES.map((us) => Math.min(us, half)));
  choices.add(currentUs);
  return [...choices].sort((a, b) => a - b);
}

/**
 * A sound layer's fields: its Volume (committed on release; sound isn't dry-run), its fades, and for music its ducking
 * under speech (on or off, and how deep). Turning ducking off keeps the layer music (0 dB, never none). Each change sets
 * the whole content, built at its turn from the layer as the project has it then, so a quick second change keeps the
 * first.
 */
export function AudioInspector({ item, content }: { item: Item; content: AudioContent }) {
  const { t } = useLingui();
  const music = typeof content.duck === 'number';
  const heard = useSpeech((s) => s.ranges.length > 0);
  const set = (label: 'clip.fades.undo' | 'music.duck.undo', patch: Partial<AudioContent>) =>
    void commitLatest(label, () => contentOps(item, content, patch));
  const fadeOptions = (currentUs: number) =>
    fadeChoices(item.durationUs, currentUs).map((us) => ({
      value: String(us),
      label:
        us === 0
          ? t({ id: 'clip.fades.none', message: 'None', comment: 'No fade' })
          : `${(us / 1_000_000).toFixed(1).replace(/\.0$/, '')} s`,
    }));
  const volume = t({ id: 'volume.label', message: 'Volume' });
  const depth = t({ id: 'music.duck.depth', message: 'How much quieter' });
  const ducking = music && (content.duck ?? 0) < 0;
  return (
    <>
      <Field label={volume}>
        <ToolSlider
          label={volume}
          commitLabel="volume.undo"
          value={content.volume}
          min={0}
          max={2}
          step={0.01}
          format={percent}
          preview={false}
          ops={(v) => contentOps(item, content, { volume: v })}
        />
      </Field>
      {music && (
        <>
          <Field label={t({ id: 'music.duck', message: 'Ducking' })} narrowStack>
            <Segmented
              label={t({ id: 'music.duck.label', message: 'Quieter while people talk' })}
              value={ducking ? 'on' : 'off'}
              options={[
                { value: 'on', label: t({ id: 'music.duck.on', message: 'On' }) },
                { value: 'off', label: t({ id: 'music.duck.off', message: 'Off' }) },
              ]}
              fill
              onChange={(on) => set('music.duck.undo', { duck: on === 'on' ? DUCK_DB : 0 })}
            />
          </Field>
          {ducking && (
            <Field label={t({ id: 'music.duck.depth.short', message: 'Quieter by' })}>
              <ToolSlider
                label={depth}
                commitLabel="music.duck.undo"
                value={-(content.duck ?? DUCK_DB)}
                min={0}
                max={-DEEPEST_DB}
                step={1}
                format={(db) => `${Math.round(db)} dB`}
                preview={false}
                ops={(db) => contentOps(item, content, { duck: -db })}
              />
            </Field>
          )}
          {ducking && !heard && (
            <p className="field-note">
              {t({
                id: 'music.duck.waiting',
                message: 'Ducking starts once Klipolo has listened to your clips.',
              })}
            </p>
          )}
        </>
      )}
      <Field label={t({ id: 'clip.fades.in', message: 'Fade in' })} stack>
        <Segmented
          label={t({ id: 'clip.fades.in', message: 'Fade in' })}
          value={String(content.fadeInUs)}
          options={fadeOptions(content.fadeInUs)}
          fill
          onChange={(v) => set('clip.fades.undo', { fadeInUs: Number(v) })}
        />
      </Field>
      <Field label={t({ id: 'clip.fades.out', message: 'Fade out' })} stack>
        <Segmented
          label={t({ id: 'clip.fades.out', message: 'Fade out' })}
          value={String(content.fadeOutUs)}
          options={fadeOptions(content.fadeOutUs)}
          fill
          onChange={(v) => set('clip.fades.undo', { fadeOutUs: Number(v) })}
        />
      </Field>
      <TimingField item={item} />
    </>
  );
}
