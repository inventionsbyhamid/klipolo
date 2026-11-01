import { Slider as S } from 'radix-ui';
import { useRef, type CSSProperties } from 'react';
import { cn } from './cn';

interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  /** Every move (preview). */
  onChange: (value: number) => void;
  /** A release, or a key press, that changed the value; `keyed` for a key press. */
  onCommit: (value: number, keyed: boolean) => void;
  /** Focus left the slider. */
  onBlur?: () => void;
  /** The value shown beside the slider (and spoken). */
  format?: (value: number) => string;
  /** What is spoken for the value, when it says more than the shown text (a hue's colour name). */
  valueText?: (value: number) => string;
  /** The track drawn as a picture of what it sets (a hue's spectrum, a colour fading out): no range fill then. */
  trackStyle?: CSSProperties;
  /** Inert, with its value still shown (why is said beside it). */
  disabled?: boolean;
  className?: string;
}

/**
 * A slider: a 4 px track, paper up to the value and press colour after, a 14 px paper knob, and the value
 * to the right in faint tabular text. Arrow keys step 1, Shift arrow 10. The knob carries the name and spoken value.
 */
export function Slider({
  label,
  value,
  min,
  max,
  step,
  onChange,
  onCommit,
  onBlur,
  format,
  valueText,
  trackStyle,
  disabled,
  className,
}: SliderProps) {
  const text = format ? format(value) : String(value);
  /** Whether the change being committed came from a key (Radix commits each key press at once). */
  const keyed = useRef(false);
  return (
    <div className={cn('slider-row', className)}>
      <S.Root
        className="slider-root"
        value={[value]}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onKeyDownCapture={() => (keyed.current = true)}
        onPointerDownCapture={() => (keyed.current = false)}
        onBlur={onBlur}
        onValueChange={([v]) => onChange(v)}
        onValueCommit={([v]) => {
          onCommit(v, keyed.current);
          keyed.current = false;
        }}
      >
        <S.Track className={cn('slider-track', trackStyle && 'slider-track-drawn')} style={trackStyle}>
          {!trackStyle && <S.Range className="slider-range" />}
        </S.Track>
        <S.Thumb
          className="slider-thumb"
          aria-label={label}
          aria-valuetext={valueText ? valueText(value) : text}
          aria-disabled={disabled || undefined}
        />
      </S.Root>
      <span className="slider-val">{text}</span>
    </div>
  );
}
