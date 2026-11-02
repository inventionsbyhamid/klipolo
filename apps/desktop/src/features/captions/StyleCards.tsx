import { useLingui } from '@lingui/react/macro';
import { useRef, type FocusEvent, type KeyboardEvent, type ReactNode } from 'react';
import type { CaptionSettings } from '../../bindings/CaptionSettings';
import type { CaptionStyleDef } from '../../bindings/CaptionStyleDef';
import { Tooltip } from '../../design/Tooltip';
import { SpriteIcon } from '../../design/SpriteIcon';
import { Icon } from '../../layout/Icon';
import type { LabelId } from '../../lib/labels';
import { defaultStyle, sampleCss, STYLE_NAMES, STYLE_ORDER, STYLE_TIPS, styleById, styleId } from '../../lib/captionStyles';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { useProject } from '../../state/project';
import { announce } from '../timeline/announce';
import { commitCaptions, setCaptionsOp } from './captionOps';

/** The arrow keys' directions: left and right step one card, up and down one row (`columnsOf` cards). */
const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -1, ArrowDown: 1 };

/** The "Off" card's id among the cards. */
const OFF = 'off';

/**
 * How many cards the grid shows in a row now: those on the first card's line. Measured at each press, never assumed.
 */
export function columnsOf(cards: readonly (HTMLElement | null)[]): number {
  const shown = cards.filter((card): card is HTMLElement => !!card);
  let n = 1;
  while (n < shown.length && shown[n].offsetTop === shown[0].offsetTop) n++;
  return n;
}

/** A new style starts from its own look and color, and shows captions (as chat's `setCaptions` style does). */
const pick = (style: string): Partial<CaptionSettings> => ({
  style,
  variant: undefined,
  accent: undefined,
  enabled: true,
});

/** A card's still: the sample words in the style, the last one drawn as the word being said. */
function Sample({ style, variant, accent }: { style: CaptionStyleDef; variant?: string; accent?: string }) {
  const { t } = useLingui();
  const css = sampleCss(style, variant, accent);
  const sample = t({
    id: 'captions.style.sample',
    message: 'Make it pop',
    comment: 'Each caption style card draws this in the style; its last word is drawn as the word being said',
  });
  const cut = sample.lastIndexOf(' ');
  return (
    <span className="cap" style={css.line}>
      {sample.slice(0, cut + 1)}
      <span style={css.said}>{sample.slice(cut + 1)}</span>
    </span>
  );
}

/**
 * The caption styles as one-tap cards: Off, then the shown styles (`STYLE_ORDER`), a radio group of 2:1
 * stills with the name under each and a check before the chosen one (not color alone). Hovering or focusing a style
 * dry-runs it on the video (the engine draws it; nothing is committed), leaving the group or Escape puts the video
 * back, and a click or Enter applies it as one undo step. "Off" is the only off switch. The arrow keys move between
 * cards, up and down by the columns the grid shows.
 */
export function StyleCards() {
  const { t, i18n } = useLingui();
  const settings = useProject((s) => s.snapshot!.project.captions);
  const formats = useProject((s) => s.snapshot!.project.settings.formats);
  const cards = useRef<(HTMLButtonElement | null)[]>([]);
  const previewing = useRef(false);
  const chosen = settings.enabled ? (settings.style ?? defaultStyle(formats)) : OFF;
  const styles = STYLE_ORDER.map((key) => ({ key, id: styleId(key), def: styleById(styleId(key)) })).filter(
    (s): s is typeof s & { def: CaptionStyleDef } => s.def !== null,
  );
  const ids = [OFF, ...styles.map((s) => s.id)];
  // Tab reaches the chosen card (the first, when the project's style isn't one shown).
  const tabStop = ids.includes(chosen) ? chosen : ids[0];

  const stop = () => {
    if (!previewing.current) return;
    previewing.current = false;
    clearPreview();
  };
  /** What picking `id` changes, as an undo step: nothing for the card already chosen. */
  const change = (id: string): [LabelId, Partial<CaptionSettings>] | null => {
    if (id === chosen) return null;
    if (id === OFF) return ['captions.off.undo', { enabled: false }];
    // Turning them on in the style they had keeps its look and color.
    if (!settings.enabled && id === (settings.style ?? defaultStyle(formats)))
      return ['captions.on.undo', { enabled: true }];
    return ['captions.style.undo', pick(id)];
  };
  const preview = (id: string) => {
    const next = change(id);
    if (!next) return stop();
    previewing.current = true;
    previewOps([setCaptionsOp(next[1])]);
  };
  const apply = (id: string) => {
    const next = change(id);
    if (!next) return;
    previewing.current = false;
    // The new revision expires the dry run: the frame never flickers back.
    dropPendingPreview();
    const turnsOn = !settings.enabled && id !== OFF;
    void commitCaptions(next[0], () => next[1]).then((ok) => {
      if (!ok) clearPreview();
      else if (turnsOn)
        announce(
          t({
            id: 'captions.on.said',
            message: 'Captions are on. Their words are below the styles.',
            comment: 'Said to screen readers when picking a caption style turns captions on',
          }),
        );
    });
  };
  const onKeyDown = (i: number) => (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === 'Escape') return stop();
    const way = STEPS[e.key];
    if (way === undefined) return;
    e.preventDefault();
    const step = e.key === 'ArrowUp' || e.key === 'ArrowDown' ? way * columnsOf(cards.current) : way;
    cards.current[Math.min(ids.length - 1, Math.max(0, i + step))]?.focus();
  };
  const onBlur = (e: FocusEvent<HTMLDivElement>) => {
    if (!(e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget))) stop();
  };
  const card = (id: string, i: number, name: string, frame: ReactNode, tip?: string) => {
    const checked = id === chosen;
    const button = (
      <button
        key={id}
        ref={(el) => {
          cards.current[i] = el;
        }}
        type="button"
        role="radio"
        aria-checked={checked}
        aria-label={name}
        tabIndex={id === tabStop ? 0 : -1}
        className="style-card"
        onPointerEnter={() => preview(id)}
        onFocus={() => preview(id)}
        onClick={() => apply(id)}
        onKeyDown={onKeyDown(i)}
      >
        {frame}
        <span className="style-name">
          {checked && (
            <span data-testid="style-chosen" className="style-check">
              <Icon name="check" />
            </span>
          )}
          {name}
        </span>
      </button>
    );
    return tip ? (
      <Tooltip key={id} label={tip}>
        {button}
      </Tooltip>
    ) : (
      button
    );
  };
  return (
    <div
      role="radiogroup"
      aria-label={t({ id: 'captions.style', message: 'Caption style' })}
      className="style-grid"
      onPointerLeave={stop}
      onBlur={onBlur}
    >
      {card(
        OFF,
        0,
        t({ id: 'captions.off', message: 'Off', comment: 'The caption style card that turns captions off' }),
        <span className="style-frame style-off">
          <SpriteIcon name="eye-off" />
        </span>,
      )}
      {styles.map(({ key, id, def }, k) => {
        const name = STYLE_NAMES[key] ? i18n._(STYLE_NAMES[key]) : def.name;
        const tip = STYLE_TIPS[key] ? i18n._(STYLE_TIPS[key]) : undefined;
        const own = settings.style === id;
        return card(
          id,
          k + 1,
          name,
          <span className="style-frame" aria-hidden>
            <Sample
              style={def}
              variant={own ? settings.variant : undefined}
              accent={own ? settings.accent : undefined}
            />
          </span>,
          tip,
        );
      })}
    </div>
  );
}
