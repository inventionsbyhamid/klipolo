import { useLingui } from '@lingui/react/macro';
import { useRef, useState, type JSX, type RefObject } from 'react';
import { sameColor, sameRgb } from '../../design/color/colorMath';
import { HexInput } from '../../design/color/HexInput';
import { focusSwatch, SwatchList, type SwatchGroup, type SwatchOption } from '../../design/color/SwatchList';
import { PopoverContent } from '../../design/Popover';
import { nearestColorName } from '../../lib/swatches';
import { useRecentColors } from '../../state/recentColors';
import { CustomPicker } from './CustomPicker';
import { EyedropperButton } from './EyedropperButton';
import { projectColorsNow } from './projectColors';

interface ColorPopoverProps {
  label: string;
  value: string | null;
  /** The colour the swatches select: `value`, without its alpha in a field that has one. */
  selected: string | null;
  /** Changes when a pick replaced the picker's keys still settling: it shows `value` again (`CustomPicker`). */
  resync: number;
  alpha: boolean;
  /** The presets, named. */
  presets: SwatchOption[];
  /** The none swatch, first among the presets, when the setting has one. */
  none: SwatchOption | null;
  onPreview(color: string | null): void;
  onPreviewEnd(): void;
  onPick(color: string | null): void;
  /** A custom colour picked: the hex field's, a Recent swatch's or the eyedropper's. Recent keeps it. */
  onCustom(hex: string): void;
  /** The custom picker's drag frames, releases, keys and cancelled drags (`CustomPicker`). */
  onDrag(hex: string): void;
  onRelease(hex: string): void;
  onKey(hex: string): void;
  onCancel(): void;
}

/**
 * The color popover: anchored to the "+" that opened it and non-modal, so the video stays in view and shows each
 * colour live. Klipolo's colours (none first), then the colours this video already uses and the person's recent custom
 * picks (each group only when it has a colour, none repeating a colour above it, both as they were when it opened), as
 * one listbox in labelled groups; then the custom picker (area, hue and, with an alpha, opacity), then a hex field,
 * and on a Mac the eyedropper beside it. It opens on the selected swatch, and scrolls on a short window.
 */
export function ColorPopover(props: ColorPopoverProps): JSX.Element {
  /** The last press inside was the pointer's: the focus it moves isn't scrolled to (the area takes it unscrolled). */
  const pressed = useRef(false);
  /** The eyedropper's loupe shows: an Escape then is the loupe's cancel, never the popover's close. */
  const picking = useRef(false);
  return (
    <PopoverContent
      aria-label={props.label}
      side="bottom"
      align="start"
      className="color-popover"
      // Open on the selected swatch (else the first), without previewing it.
      onOpenAutoFocus={(e) => {
        e.preventDefault();
        if (e.currentTarget instanceof Element) focusSwatch(e.currentTarget);
      }}
      onPointerDownCapture={() => {
        pressed.current = true;
      }}
      onKeyDownCapture={() => {
        pressed.current = false;
      }}
      // Its Tab loop moves focus without scrolling: the swatch or field focus lands on comes into view.
      onFocus={(e) => {
        if (!pressed.current) e.target.scrollIntoView?.({ block: 'nearest' });
      }}
      onEscapeKeyDown={(e) => {
        if (picking.current) e.preventDefault();
      }}
    >
      <PopoverBody {...props} picking={picking} />
    </PopoverContent>
  );
}

/**
 * "In this video" and "Recent" as they were when the popover opened: read once, with no subscription, so nothing
 * re-sorts, comes or goes under the pointer or focus while it is open (a pick moves only the selection ring), and a
 * change made elsewhere shows at the next open. Recent leaves out the video's colours, and both leave out what the
 * presets offer, so a colour shows once.
 */
function useOpeningColors(shown: readonly string[]): { video: string[]; recent: string[] } {
  const [opened] = useState(() => {
    const video = projectColorsNow(shown);
    const recent = useRecentColors.getState().recent.filter((c) => !video.some((v) => sameRgb(v, c)));
    return { video, recent };
  });
  const fresh = (c: string) => !shown.some((s) => sameRgb(s, c));
  return { video: opened.video.filter(fresh), recent: opened.recent.filter(fresh) };
}

/** What the popover holds: mounted while it is open, so it reads the opening colours once per open. */
function PopoverBody({
  value,
  selected,
  resync,
  alpha,
  presets,
  none,
  onPreview,
  onPreviewEnd,
  onPick,
  onCustom,
  onDrag,
  onRelease,
  onKey,
  onCancel,
  picking,
}: ColorPopoverProps & { picking: RefObject<boolean> }): JSX.Element {
  const { t, i18n } = useLingui();
  const shown = presets.flatMap((o) => (o.value === null ? [] : [o.value]));
  const { video, recent } = useOpeningColors(shown);
  const word = (name: string, n: number) =>
    t({
      id: 'color.numbered',
      message: `${name} ${n}`,
      comment: '{name} is a color’s name; {n} tells apart two swatches with the same name, e.g. “Yellow 2”',
    });
  const named = (hexes: string[]): SwatchOption[] =>
    hexes.map((hex) => ({ value: hex, name: i18n._(nearestColorName(hex)) }));
  const groups: SwatchGroup[] = [
    {
      id: 'klipolo',
      label: t({ id: 'color.group.klipolo', message: 'Klipolo', comment: 'The app’s own preset colors' }),
      options: [...(none ? [none] : []), ...presets],
    },
    ...(video.length > 0
      ? [
          {
            id: 'video',
            label: t({
              id: 'color.group.video',
              message: 'In this video',
              comment: 'Colors this video already uses (its titles, captions and effects)',
            }),
            options: numbered(named(video), word),
          },
        ]
      : []),
    ...(recent.length > 0
      ? [
          {
            id: 'recent',
            label: t({
              id: 'color.group.recent',
              message: 'Recent',
              comment: 'The custom colors the person picked last, in any project',
            }),
            options: numbered(named(recent), word),
          },
        ]
      : []),
  ];
  return (
    <>
      <SwatchList
        label={t({ id: 'color.swatches', message: 'Colors', comment: 'The color swatches in the color popover' })}
        groups={groups}
        value={selected}
        onPreview={onPreview}
        onPreviewEnd={onPreviewEnd}
        // A Recent swatch is a custom colour again (it comes first in Recent); Recent repeats no other swatch.
        onPick={(c) => (c !== null && recent.some((r) => sameColor(r, c)) ? onCustom(c) : onPick(c))}
      />
      <CustomPicker
        value={value}
        resync={resync}
        alpha={alpha}
        onDrag={onDrag}
        onRelease={onRelease}
        onKey={onKey}
        onCancel={onCancel}
      />
      {/* The hex field, and on a Mac the eyedropper beside it (a picked colour is a custom one, as a typed hex). */}
      <div className="hex-row">
        <HexInput
          label={t({ id: 'color.hex', message: 'Hex color' })}
          value={value}
          alpha={alpha}
          invalidText={
            alpha
              ? t({
                  id: 'color.hex.invalid.alpha',
                  message: 'Type 3 or 6 hex digits, or 8 with opacity, like FFD02E80.',
                  comment: 'A color setting with opacity: the last two of 8 digits are the opacity',
                })
              : t({ id: 'color.hex.invalid', message: 'Type 3 or 6 hex digits, like FFD02E.' })
          }
          noAlphaText={t({
            id: 'color.hex.noAlpha',
            message: 'This setting has no opacity, so the color is used without it.',
            comment: 'Eight hex digits (a color with opacity) went into a color setting that has none',
          })}
          onCommit={onCustom}
        />
        <EyedropperButton onPick={onCustom} running={picking} />
      </div>
    </>
  );
}

/** Swatches that would share a name are told apart by a number: "Yellow", "Yellow 2". */
function numbered(options: SwatchOption[], word: (name: string, n: number) => string): SwatchOption[] {
  const seen = new Map<string, number>();
  return options.map((option) => {
    const n = (seen.get(option.name) ?? 0) + 1;
    seen.set(option.name, n);
    return n === 1 ? option : { ...option, name: word(option.name, n) };
  });
}
