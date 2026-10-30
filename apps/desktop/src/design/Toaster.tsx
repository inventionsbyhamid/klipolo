import { Trans, useLingui } from '@lingui/react/macro';
import { Toast } from 'radix-ui';
import { useRef } from 'react';
import { Icon } from '../layout/Icon';
import { useToasts, type ToastMessage } from '../state/toasts';

/** Radix puts the shortcut that focuses the toasts (F8) where its label says `{hotkey}`. */
const HOTKEY = '{hotkey}';

/** A toast stays 5 s, 10 s when it offers an action; hovering or focusing it pauses the time. */
const SHOWN_MS = 5000;
const WITH_ACTION_MS = 10000;

/**
 * An open menu, popover or dialog. Only Radix's topmost layer hears Escape, so a persistent toast that appeared over
 * one gives way to an Escape pressed in it (it closes, as any toast would); keeping the key would leave the layer
 * stuck.
 */
const LAYER = '[role="dialog"], [role="alertdialog"], [data-radix-popper-content-wrapper]';

/** Results of hand actions, bottom left over the timeline: one short sentence and at most one action each. */
export function Toaster() {
  const { t } = useLingui();
  const toasts = useToasts((s) => s.toasts);
  const dismiss = useToasts((s) => s.dismiss);
  return (
    <Toast.Provider
      duration={SHOWN_MS}
      label={t({
        id: 'toast.label',
        message: 'Notification',
        comment: 'Screen readers say it before each toast',
      })}
    >
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} onDismiss={dismiss} />
      ))}
      <Toast.Viewport
        label={t({
          id: 'toast.region',
          message: `Notifications (${HOTKEY})`,
          comment: 'The toasts’ region; keep {HOTKEY}, it becomes the shortcut that focuses them (F8)',
        })}
        className="toasts"
      />
    </Toast.Provider>
  );
}

function ToastItem({ toast: t, onDismiss }: { toast: ToastMessage; onDismiss: (id: number) => void }) {
  const node = useRef<HTMLLIElement>(null);
  /** Radix is closing this persistent toast for an Escape pressed elsewhere: the close is ignored. */
  const keep = useRef(false);
  const error = t.kind === 'error';
  return (
    <Toast.Root
      ref={node}
      open
      // A notice the person has to deal with waits for them; Radix pauses the others while hovered or focused.
      duration={t.persistent ? Infinity : t.actions?.length ? WITH_ACTION_MS : undefined}
      // Radix closes the topmost toast on any Escape; a persistent one closes only on an Escape inside it.
      onEscapeKeyDown={
        t.persistent
          ? (e) => {
              const target = e.target instanceof Element ? e.target : null;
              if (target && (node.current?.contains(target) || target.closest(LAYER))) return;
              keep.current = true;
              queueMicrotask(() => {
                keep.current = false;
              });
            }
          : undefined
      }
      type={error ? 'foreground' : 'background'}
      onOpenChange={(open) => !open && !keep.current && onDismiss(t.id)}
      className="toast"
    >
      <Icon name={t.done ? 'check' : 'info'} />
      <Toast.Description className="toast-text">
        {error && (
          <span className="sr-only">
            <Trans id="toast.error" comment="Read before an error message by screen readers">
              Error:
            </Trans>{' '}
          </span>
        )}
        {t.text}
      </Toast.Description>
      {t.actions?.map((action) => (
        // Its own words for screen readers too; the toast closes as it runs (Radix's Action is a Close).
        <Toast.Action key={action.label} altText={action.label} asChild>
          <button type="button" className="btn btn-ghost btn-sm" onClick={action.run}>
            {action.label}
          </button>
        </Toast.Action>
      ))}
    </Toast.Root>
  );
}
