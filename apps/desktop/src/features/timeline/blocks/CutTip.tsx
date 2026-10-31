import { memo, useEffect, useRef } from 'react';

/** As long as the app's tooltips wait (`TooltipProvider`). */
const TIP_DELAY_MS = 400;

const tipOf = (target: EventTarget | null) =>
  target instanceof Element ? target.closest<HTMLElement>('[data-tip]') : null;

/**
 * The one tooltip of a lane's cut markers (`CutMarker`'s `data-tip`), drawn above the marker pointed at (after the
 * tooltips' delay) or focused from the keyboard, the way `Tooltip` draws. One element and a few listeners on the lane,
 * written to directly: a Radix tooltip per marker would cost as much to mount as the rest of the lane. Visual only
 * (`aria-hidden`): each marker's name already says what its tip does.
 */
export const CutTip = memo(function CutTip() {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const tip = ref.current;
    const lane = tip?.parentElement;
    if (!tip || !lane) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    /** A press focuses its marker: that focus shows no tip (a click hides it, as `Tooltip` does). */
    let pressed = false;
    const hide = () => {
      clearTimeout(timer);
      tip.hidden = true;
    };
    const show = (el: HTMLElement, delay: number) => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        if (!el.isConnected) return;
        tip.textContent = el.dataset.tip ?? '';
        tip.style.left = `${el.offsetLeft + el.offsetWidth / 2}px`;
        tip.style.top = `${el.offsetTop - 6}px`;
        tip.hidden = false;
      }, delay);
    };
    const over = (e: PointerEvent) => {
      const el = tipOf(e.target);
      if (el) show(el, TIP_DELAY_MS);
    };
    const out = (e: PointerEvent) => {
      if (tipOf(e.target) !== tipOf(e.relatedTarget)) hide();
    };
    const down = (e: PointerEvent) => {
      // Only a marker's own press focuses it (`pressMarker`).
      pressed = e.button === 0 && tipOf(e.target) !== null;
      hide();
    };
    const focusIn = (e: FocusEvent) => {
      const el = tipOf(e.target);
      if (el && !pressed) show(el, 0);
      pressed = false;
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') hide();
    };
    lane.addEventListener('pointerover', over);
    lane.addEventListener('pointerout', out);
    // Capture: a marker's press stops its event at the marker (it never reaches the lane, which seeks).
    lane.addEventListener('pointerdown', down, true);
    lane.addEventListener('focusin', focusIn);
    lane.addEventListener('focusout', hide);
    lane.addEventListener('keydown', key);
    return () => {
      hide();
      lane.removeEventListener('pointerover', over);
      lane.removeEventListener('pointerout', out);
      lane.removeEventListener('pointerdown', down, true);
      lane.removeEventListener('focusin', focusIn);
      lane.removeEventListener('focusout', hide);
      lane.removeEventListener('keydown', key);
    };
  }, []);
  return <div ref={ref} hidden aria-hidden data-cut-tip className="tl-cut-tip" />;
});
