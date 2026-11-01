import { useRef, useState, type FocusEvent, type JSX, type KeyboardEvent, type PointerEvent } from 'react';

export type Channel = 'saturation' | 'brightness';
/** A point in the area: saturation left to right, brightness bottom to top, each 0–100. */
export interface Sv {
  saturation: number;
  brightness: number;
}

interface ColorAreaProps {
  /** 0–360°: the area's colour at full saturation and brightness. */
  hue: number;
  saturation: number;
  brightness: number;
  labels: { saturation: string; brightness: string };
  /** What a screen reader says for a channel's value (in words, from the catalogue). */
  valueText(channel: Channel, value: number): string;
  /** A press or a move of the pointer: dry-run it. */
  onDrag(sv: Sv): void;
  /** The pointer let go: one pick. */
  onRelease(sv: Sv): void;
  /** A key or an assistive-technology step: settles into one pick. */
  onKey(sv: Sv): void;
  /** The drag ended without a release (a cancelled pointer, lost capture): put the colour back. */
  onCancel(): void;
}

/** The keys the area takes: which channel each changes, and by how much (Shift: ten times the arrows) or to what. */
const KEYS: Record<string, { channel: Channel; by?: number; to?: number }> = {
  ArrowLeft: { channel: 'saturation', by: -1 },
  ArrowRight: { channel: 'saturation', by: 1 },
  ArrowDown: { channel: 'brightness', by: -1 },
  ArrowUp: { channel: 'brightness', by: 1 },
  Home: { channel: 'saturation', to: 0 },
  End: { channel: 'saturation', to: 100 },
  PageDown: { channel: 'brightness', by: -10 },
  PageUp: { channel: 'brightness', by: 10 },
};
const ARROWS: Record<string, true> = { ArrowLeft: true, ArrowRight: true, ArrowDown: true, ArrowUp: true };

const clamp01 = (n: number) => Math.min(1, Math.max(0, n));
const clamp100 = (n: number) => Math.min(100, Math.max(0, n));

/**
 * The saturation/brightness area of the color popover. A press or drag anywhere picks the point under
 * the pointer (captured, so the drag may leave the area); it dry-runs while moving and is one pick on release. For the
 * keyboard and assistive technology it is two visually hidden sliders, Saturation and Brightness, one tab stop between
 * them (it follows focus: the channel a key last changed). ←/→ change saturation and ↑/↓ brightness (Shift: by 10),
 * Home and End set saturation to 0 and 100, Page Up and Page Down change brightness by 10; Tab, Escape and modified
 * keys pass through (Escape closes the popover). A VoiceOver or Voice Control step changes the hidden slider itself
 * (no key), and is a key step too.
 */
export function ColorArea(p: ColorAreaProps): JSX.Element {
  const { hue, saturation, brightness, labels, valueText } = p;
  const saturationInput = useRef<HTMLInputElement>(null);
  const brightnessInput = useRef<HTMLInputElement>(null);
  const inputOf = (channel: Channel) => (channel === 'saturation' ? saturationInput : brightnessInput);
  /** The pointer dragging, while one does. */
  const dragging = useRef<number | null>(null);
  /** The channel focus is on, while it is in the area (the tab stop); null once it has left. */
  const [focused, setFocused] = useState<Channel | null>(null);
  const tabStop = focused ?? 'saturation';

  const at = (e: PointerEvent<HTMLDivElement>): Sv => {
    const r = e.currentTarget.getBoundingClientRect();
    const x = r.width > 0 ? clamp01((e.clientX - r.left) / r.width) : 0;
    const y = r.height > 0 ? clamp01((e.clientY - r.top) / r.height) : 0;
    return { saturation: x * 100, brightness: (1 - y) * 100 };
  };
  /** The drag by `e`'s pointer ends here; false when no drag was under way. */
  const end = (e: PointerEvent<HTMLDivElement>) => {
    if (dragging.current !== e.pointerId) return false;
    dragging.current = null;
    return true;
  };
  const step = (channel: Channel, value: number) => {
    const sv = { saturation, brightness, [channel]: clamp100(value) };
    p.onKey(sv);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const key = KEYS[e.key];
    if (!key) return;
    e.preventDefault();
    const now = Math.round(key.channel === 'saturation' ? saturation : brightness);
    const by = (key.by ?? 0) * (e.shiftKey && ARROWS[e.key] ? 10 : 1);
    // Focus follows the channel that changed, so its slider speaks the new value.
    inputOf(key.channel).current?.focus();
    step(key.channel, key.to ?? now + by);
  };
  const onBlur = (e: FocusEvent<HTMLDivElement>) => {
    if (e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget)) return;
    setFocused(null);
  };

  const input = (channel: Channel) => {
    const value = channel === 'saturation' ? saturation : brightness;
    return (
      <input
        ref={inputOf(channel)}
        type="range"
        min={0}
        max={100}
        step={1}
        className="sr-only"
        aria-label={labels[channel]}
        aria-valuetext={valueText(channel, value)}
        value={Math.round(value)}
        tabIndex={tabStop === channel ? 0 : -1}
        onFocus={() => setFocused(channel)}
        // VoiceOver's and Voice Control's increments change the value without a key.
        onChange={(e) => step(channel, Number(e.target.value))}
      />
    );
  };

  return (
    <div
      data-testid="color-area"
      className="color-area"
      // The hue the area shows is the colour being picked: data, not a token.
      style={{
        background: `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, hsl(${hue} 100% 50%))`,
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        // No text selection, and focus stays on the saturation slider rather than moving to the popover.
        e.preventDefault();
        dragging.current = e.pointerId;
        e.currentTarget.setPointerCapture(e.pointerId);
        saturationInput.current?.focus({ preventScroll: true });
        p.onDrag(at(e));
      }}
      onPointerMove={(e) => {
        if (dragging.current === e.pointerId) p.onDrag(at(e));
      }}
      onPointerUp={(e) => {
        if (end(e)) p.onRelease(at(e));
      }}
      onPointerCancel={(e) => {
        if (end(e)) p.onCancel();
      }}
      // Capture lost without a pointer-up (the window lost it): nothing was let go, so nothing is picked.
      onLostPointerCapture={(e) => {
        if (end(e)) p.onCancel();
      }}
      onKeyDown={onKeyDown}
      onBlur={onBlur}
    >
      <span
        aria-hidden
        className="color-area-knob"
        style={{ left: `${saturation}%`, bottom: `${brightness}%` }}
      />
      {input('saturation')}
      {input('brightness')}
    </div>
  );
}
