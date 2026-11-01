import { useLingui } from '@lingui/react/macro';
import { useRef, type CSSProperties, type KeyboardEvent } from 'react';
import { Icon } from '../../layout/Icon';
import { contribLabel, isInstalled } from '../../lib/contrib';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { applyLook, LOOKS, lookTargets, presetName, sharedPreset, type LookChoice } from './looks';
import './looks.css';

/** The cards are two to a row: up and down step two. */
const COLUMNS = 2;
const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -COLUMNS, ArrowDown: COLUMNS };

/**
 * The Looks view of the left panel: None and the built-in looks as cards in two columns, a swatch of each look's colour
 * with its name under it and a check before the one the clips have (not color alone). A click or Enter puts it on the
 * selected clip, or on every clip while no clip is selected, as one undo step; the line under the cards says which.
 * Clips with different looks show none chosen. Without clips the cards are off. A look this version lacks is named in a
 * note, with its id as a tooltip. The arrow keys move between the cards.
 */
export function LooksPanel() {
  const { t } = useLingui();
  const project = useProject((s) => s.snapshot?.project);
  const selectedId = useSelection((s) => s.selectedId);
  const cards = useRef<(HTMLButtonElement | null)[]>([]);
  if (!project) return null;
  const targets = lookTargets(project, selectedId);
  const selectedClip = targets.length === 1 && targets[0].id === selectedId;
  const current = sharedPreset(targets);
  // Tab reaches the chosen card (the first, when the clips' looks differ).
  const chosen = LOOKS.findIndex((look) => look.preset === current);
  const tabStop = Math.max(0, chosen);
  const choose = (look: LookChoice) => {
    // The look they all have already: no empty undo step.
    if (current !== look.preset) void applyLook(targets, look);
  };
  const onKeyDown = (i: number) => (e: KeyboardEvent<HTMLButtonElement>) => {
    const step = STEPS[e.key];
    if (step === undefined) return;
    e.preventDefault();
    cards.current[Math.min(LOOKS.length - 1, Math.max(0, i + step))]?.focus();
  };
  return (
    <div className="looks-scroll">
      <div role="radiogroup" aria-label={t({ id: 'looks.label', message: 'Look' })} className="look-grid">
        {LOOKS.map((look, i) => {
          const checked = i === chosen;
          const name = presetName(look.preset);
          const swatch = { '--look-from': look.swatch[0], '--look-to': look.swatch[1] } as CSSProperties;
          return (
            <button
              key={look.key}
              ref={(el) => {
                cards.current[i] = el;
              }}
              type="button"
              role="radio"
              aria-checked={checked}
              aria-label={name}
              tabIndex={i === tabStop ? 0 : -1}
              disabled={targets.length === 0}
              className="look-card"
              onClick={() => choose(look)}
              onKeyDown={onKeyDown(i)}
            >
              <span className="look-swatch" style={swatch} aria-hidden />
              <span className="look-name">
                {checked && (
                  <span className="look-check">
                    <Icon name="check" />
                  </span>
                )}
                {name}
              </span>
            </button>
          );
        })}
      </div>
      {current && !isInstalled(current) && <MissingLook preset={current} />}
      <p className="field-note">
        {targets.length === 0
          ? t({ id: 'looks.hint.noVideo', message: 'Add a video to try a look.' })
          : selectedClip
            ? t({ id: 'looks.hint.selectedClip', message: 'Applies to the selected clip.' })
            : t({ id: 'looks.hint.everyClip', message: 'Applies to every clip.' })}
      </p>
    </div>
  );
}

/** A look the clips have that this version lacks: named, with its id as a tooltip. */
function MissingLook({ preset }: { preset: string }) {
  const { t } = useLingui();
  const name = contribLabel(preset, 'look');
  return (
    <p className="field-note" title={preset}>
      {t({
        id: 'looks.notInstalled',
        message: `${name}: this look isn’t installed, so the video shows without it.`,
        comment: '{name} reads e.g. “Missing look”',
      })}
    </p>
  );
}
