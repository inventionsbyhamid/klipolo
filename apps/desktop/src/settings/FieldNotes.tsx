import { Icon } from '../layout/Icon';

/** Why something in Settings didn't work, in one sentence, said at once (errors are written, not colored). */
export function FieldError({ id, text }: { id: string; text: string }) {
  return (
    <p className="field-error" id={id} role="alert">
      <Icon name="info" />
      {text}
    </p>
  );
}

/** Something to know about a setting, in one sentence. */
export function Note({ text }: { text: string }) {
  return (
    <p className="field-note" role="status">
      <Icon name="info" />
      {text}
    </p>
  );
}
