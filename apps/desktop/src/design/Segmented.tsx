import type { LucideIcon } from 'lucide-react';
import { ToggleGroup } from 'radix-ui';
import { cn } from './cn';
import { Tooltip } from './Tooltip';

interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  /** Says more than the label, e.g. what "Fill" does. */
  tooltip?: string;
  /** Drawn instead of the label, which then names the option and is its tooltip. */
  icon?: LucideIcon;
  /** Can't be chosen now (a fade longer than the clip); `describedBy` names the note that says why. */
  disabled?: boolean;
  /** Id of a note that explains the option, e.g. why it is off. */
  describedBy?: string;
}

interface SegmentedProps<T extends string> {
  label: string;
  /** The option chosen; null while none is (none shows as chosen). */
  value: T | null;
  options: SegmentedOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** Options share the row's width. */
  fill?: boolean;
}

/**
 * A segmented control: a sunken track with 2 px padding, small options in ink-soft, the checked one a signal wash with
 * a signal-ink ring. A radio group: the arrow keys move. An option with an icon shows it alone, named by its label.
 */
export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  fill = false,
}: SegmentedProps<T>) {
  return (
    <ToggleGroup.Root
      type="single"
      aria-label={label}
      value={value ?? ''}
      disabled={disabled}
      onValueChange={(next) => next && onChange(next as T)}
      className={cn('seg seg-sm', fill && 'seg-fill')}
    >
      {options.map(({ value: option, label: name, tooltip, icon: OptionIcon, disabled: off, describedBy }) => {
        // Styled from aria-checked, not data-state: a tooltip trigger sets its own data-state on the same element.
        const item = (
          <ToggleGroup.Item
            key={option}
            value={option}
            disabled={off}
            aria-describedby={describedBy}
            aria-label={OptionIcon ? name : undefined}
            className={OptionIcon ? 'seg-icon' : undefined}
          >
            {OptionIcon ? <OptionIcon size={16} aria-hidden /> : name}
          </ToggleGroup.Item>
        );
        const tip = tooltip ?? (OptionIcon ? name : undefined);
        return tip ? (
          <Tooltip key={option} label={tip}>
            {item}
          </Tooltip>
        ) : (
          item
        );
      })}
    </ToggleGroup.Root>
  );
}
