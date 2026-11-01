import { useLingui } from '@lingui/react/macro';
import { useRef, useState, type JSX } from 'react';
import { ColorArea, type Sv } from '../../design/color/ColorArea';
import { alphaOf, hexToHsv, hsvToHex, sameColor, withAlpha, type Hsv } from '../../design/color/colorMath';
import { Slider } from '../../design/Slider';
import { useIntlLocale } from '../../i18n/react';
import { formatNumber, formatPercent } from '../../lib/format';
import { nearestColorName } from '../../lib/swatches';

interface CustomPickerProps {
  /** `#RRGGBB(AA)`; null (none, or the style's own) starts from white. */
  value: string | null;
  /** The setting takes an alpha: an Opacity slider, and every colour comes out as `#RRGGBBAA`. */
  alpha: boolean;
  /** A drag frame: dry-run it. */
  onDrag(hex: string): void;
  /** A drag let go: one pick. */
  onRelease(hex: string): void;
  /** A key or an assistive-technology step: settles into one pick. */
  onKey(hex: string): void;
  /** A drag ended without a release: put the video back. */
  onCancel(): void;
  /** Changes when a pick elsewhere replaced keys still settling here: show `value` again, whatever it is. */
  resync?: number;
}

/** The colour as the picker holds it. */
interface Held {
  hsv: Hsv;
  /** The alpha's two hex digits (`OPAQUE` when the value has none), kept as they are until Opacity changes them. */
  alpha: string;
}
interface PickerState extends Held {
  /** The value it last took in, and the `resync` with it. */
  base: string | null;
  resync: number;
  /** A drag is under way: a value changed elsewhere meanwhile doesn't move the controls under the pointer. */
  dragging: boolean;
}

const WHITE: Hsv = { h: 0, s: 0, v: 100 };
/** The hue track: the spectrum, in seven stops. */
const HUE_TRACK = { background: 'linear-gradient(to right, #F00, #FF0, #0F0, #0FF, #00F, #F0F, #F00)' };
/** What shows through a see-through colour. */
const CHECKER = 'repeating-conic-gradient(var(--color-line) 0% 25%, transparent 0% 50%) 0 0 / 8px 8px';

/** The hue's visible and spoken value: the `Intl` degree unit ("240°"). */
const DEGREES: Intl.NumberFormatOptions = { style: 'unit', unit: 'degree', unitDisplay: 'narrow' };

/** The alpha digits of `percent` opacity. */
const digits = (percent: number) => withAlpha('#000000', percent).slice(7);
const OPAQUE = digits(100);

/** `value` as the picker holds it. A grey keeps `from`'s hue (it has none of its own), black its saturation too. */
function take(value: string | null, from: Hsv): Held {
  if (!value) return { hsv: from, alpha: OPAQUE };
  const { h, s, v } = hexToHsv(value);
  return {
    hsv: { h: s === 0 || v === 0 ? from.h : h, s: v === 0 ? from.s : s, v },
    alpha: value.length === 9 ? value.slice(7).toUpperCase() : OPAQUE,
  };
}

/**
 * The custom colour picker of the colour popover: a saturation/brightness area, a hue slider and, when
 * the setting has an alpha, an Opacity slider. Drags dry-run (`onDrag`; the engine draws one a frame) and are one
 * pick on release; keys and assistive-technology steps go to `onKey`, which settles them into one pick. With an alpha
 * every colour comes out as `#RRGGBBAA`, its alpha untouched unless Opacity moved: so Opacity is the only control here
 * that changes the opacity. Only the picker re-renders while a drag moves. It follows `value` when that changes from
 * elsewhere (an undo, chat or MCP while the popover is open), except under a drag.
 */
export function CustomPicker(p: CustomPickerProps): JSX.Element {
  const { value, alpha, resync = 0 } = p;
  const { t, i18n } = useLingui();
  const locale = useIntlLocale();
  const colorOf = (held: Held) => (alpha ? `${hsvToHex(held.hsv)}${held.alpha}` : hsvToHex(held.hsv));
  const [state, setState] = useState<PickerState>(() => ({
    base: value,
    resync,
    dragging: false,
    ...take(value, WHITE),
  }));
  if (state.base !== value || state.resync !== resync) {
    // A pick of ours landing (the picker already shows it), or a change from elsewhere (shown, unless a drag is on).
    // A pick that replaced our keys (`resync`) is shown whatever it is: it may be the colour the setting already had.
    const keep =
      state.resync === resync && ((value !== null && sameColor(value, colorOf(state))) || state.dragging);
    setState(
      keep
        ? { ...state, base: value }
        : { ...state, base: value, resync, dragging: false, ...take(value, state.hsv) },
    );
  }
  const { hsv } = state;
  /**
   * A drag is under way, for the event handlers: Radix commits a slider drag in the same event as its pointer-up, so
   * state would be a render behind there.
   */
  const dragging = useRef(false);

  /** Shows `next` and hands its colour to `send`; `drag` until the drag lets go. */
  const show = (next: Held, drag: boolean, send: (hex: string) => void) => {
    dragging.current = drag;
    setState((s) => ({ ...s, ...next, dragging: drag }));
    send(colorOf(next));
  };
  const cancel = () => {
    dragging.current = false;
    setState((s) => ({ ...s, ...take(value, s.hsv), dragging: false }));
    p.onCancel();
  };
  /** A slider drag Radix didn't commit (it ended where it began) or lost its pointer: put the colour back. */
  const endSlide = () => {
    if (dragging.current) cancel();
  };
  const at = (sv: Sv): Held => ({
    hsv: { h: hsv.h, s: sv.saturation, v: sv.brightness },
    alpha: state.alpha,
  });
  const hued = (h: number): Held => ({ hsv: { ...hsv, h }, alpha: state.alpha });
  const faded = (percent: number): Held => ({ hsv, alpha: digits(percent) });

  const rgb = hsvToHex(hsv);
  const name = i18n._(nearestColorName(rgb));
  const channelText = (v: number) => {
    const percent = formatPercent(locale, Math.round(v) / 100);
    return t({
      id: 'color.channel.value',
      message: `${percent}, ${name}`,
      comment:
        'Spoken for the colour area’s saturation or brightness: {percent} is formatted (“80%”), then the nearest colour’s name',
    });
  };
  /** The hue the slider holds (0–359°: 359.6° is 359, never 360). */
  const hue = Math.min(359, Math.round(hsv.h));
  const degrees = formatNumber(locale, hue, DEGREES);
  const hueText = t({
    id: 'color.hue.value',
    message: `${degrees}, ${name}`,
    comment:
      'Spoken for the hue slider: {degrees} round the colour wheel, formatted (“240°”), then the nearest colour’s name',
  });

  return (
    <div className="custom-picker">
      <ColorArea
        hue={hsv.h}
        saturation={hsv.s}
        brightness={hsv.v}
        labels={{
          saturation: t({ id: 'color.saturation', message: 'Saturation' }),
          brightness: t({ id: 'color.brightness', message: 'Brightness' }),
        }}
        valueText={(_, v) => channelText(v)}
        onDrag={(sv) => show(at(sv), true, p.onDrag)}
        onRelease={(sv) => show(at(sv), false, p.onRelease)}
        onKey={(sv) => show(at(sv), false, p.onKey)}
        onCancel={cancel}
      />
      <div
        className="custom-picker-sliders"
        // Radix commits nothing for a drag that ends where it began, and has no cancel: such a drag (its last frame a
        // rounded dry run) puts the colour back here, after Radix's own pointer-up.
        onPointerUp={endSlide}
        onPointerCancel={endSlide}
        onLostPointerCapture={endSlide}
      >
        <Slider
          label={t({ id: 'color.hue', message: 'Hue' })}
          value={hue}
          min={0}
          max={359}
          step={1}
          trackStyle={HUE_TRACK}
          format={(v) => formatNumber(locale, v, DEGREES)}
          valueText={() => hueText}
          onChange={(h) => show(hued(h), true, p.onDrag)}
          onCommit={(h, keyed) => show(hued(h), false, keyed ? p.onKey : p.onRelease)}
        />
        {alpha && (
          <Slider
            label={t({ id: 'color.opacity', message: 'Opacity' })}
            value={alphaOf(colorOf(state))}
            min={0}
            max={100}
            step={1}
            // The colour being picked fades over the checks: data, not a token.
            trackStyle={{ background: `linear-gradient(90deg, ${rgb}00, ${rgb}), ${CHECKER}` }}
            format={(v) => formatPercent(locale, v / 100)}
            onChange={(v) => show(faded(v), true, p.onDrag)}
            onCommit={(v, keyed) => show(faded(v), false, keyed ? p.onKey : p.onRelease)}
          />
        )}
      </div>
    </div>
  );
}
