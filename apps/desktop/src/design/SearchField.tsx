import type { Ref } from 'react';
import { cn } from './cn';
import { SpriteIcon } from './SpriteIcon';

interface SearchFieldProps {
  value: string;
  onChange: (value: string) => void;
  /** Its name, shown as the placeholder while it is empty. */
  label: string;
  className?: string;
  ref?: Ref<HTMLInputElement>;
}

/**
 * The search field: a 28 px raised field with 6 px corners and a search icon that filters as you type. Escape empties
 * it; the editor's keys never run from it (the shortcuts leave fields alone).
 */
export function SearchField({ value, onChange, label, className, ref }: SearchFieldProps) {
  return (
    <label className={cn('search', className)}>
      <SpriteIcon name="search" />
      <input
        ref={ref}
        type="search"
        value={value}
        placeholder={label}
        aria-label={label}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== 'Escape' || !value || e.nativeEvent.isComposing) return;
          // Only the field's own words go: a dialog or the editor around it doesn't take this Escape as well.
          e.preventDefault();
          e.stopPropagation();
          onChange('');
        }}
      />
    </label>
  );
}
