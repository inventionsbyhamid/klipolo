import { useLingui } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import type { McpStatus } from '../bindings/McpStatus';
import { mcpStatus, onMcpApps, setMcpEnabled } from '../bridge/mcp';
import { activeIntlLocale } from '../i18n';
import { Icon } from '../layout/Icon';
import { errorMessage } from '../lib/errors';
import { FieldError, Note } from './FieldNotes';

/** "Example Client and Muse Spark agent", "A, B, and C": names joined as the language shown joins them. */
function names(apps: string[]): string {
  return new Intl.ListFormat(activeIntlLocale(), { style: 'long', type: 'conjunction' }).format(apps);
}

/**
 * Settings' "Other apps": the switch that lets other apps (an AI agent, any MCP app) edit the open project, whether
 * Klipolo listens and which apps used it recently (an app that quit can stay listed about 10 minutes, so "recently"),
 * and Copy config, which puts on the clipboard what an app needs to start Klipolo's bridge. A copy of Klipolo still on
 * its disk image (or one macOS moved aside) can't be named in a config: it says to move it to Applications first.
 * Cuts an app asks for wait for the person's Apply in chat.
 */
export function OtherApps() {
  const { t } = useLingui();
  const [status, setStatus] = useState<McpStatus | null>(null);
  /** The switch as pressed, while Klipolo starts or stops listening. */
  const [wanted, setWanted] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<'done' | 'failed' | null>(null);

  useEffect(() => {
    let live = true;
    mcpStatus().then(
      (s) => live && setStatus(s),
      () => undefined,
    );
    // While Settings is open, the list follows apps as they come and go.
    const stop = onMcpApps((apps) => live && setStatus((s) => s && { ...s, connected: apps })).catch(() => null);
    return () => {
      live = false;
      void stop.then((unlisten) => unlisten?.());
    };
  }, []);

  const busy = wanted !== null;
  const on = wanted ?? status?.enabled ?? false;
  const flip = async () => {
    if (busy || !status) return;
    const next = !status.enabled;
    setWanted(next);
    setError(null);
    try {
      setStatus(await setMcpEnabled(next));
    } catch (e) {
      // Klipolo couldn't start listening: it stays off, and says so.
      setError(errorMessage(e));
    } finally {
      setWanted(null);
    }
  };
  const copy = async () => {
    if (!status || status.needsMove) return;
    try {
      await navigator.clipboard.writeText(status.config);
      setCopied('done');
    } catch {
      setCopied('failed');
    }
  };

  const said = !status
    ? ''
    : !status.enabled
      ? t({ id: 'mcp.status.off', message: 'Off.' })
      : status.connected.length === 0
        ? t({ id: 'mcp.status.none', message: 'On. No app connected yet.' })
        : t({
            id: 'mcp.status.used',
            message: `On. Recently used by ${names(status.connected)}.`,
            comment: '{0} is the names of the apps, e.g. "Example Client and Muse Spark agent"',
          });

  return (
    <section className="other-apps" aria-labelledby="settings-apps-title">
      <h3 className="section-title" id="settings-apps-title">
        {t({ id: 'mcp.title', message: 'Other apps' })}
      </h3>
      <div className="switch-row">
        <label htmlFor="settings-apps">{t({ id: 'mcp.switch', message: 'Let other apps edit with Klipolo' })}</label>
        <button
          id="settings-apps"
          type="button"
          role="switch"
          className="settings-switch"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on.
          tabIndex={0}
          aria-checked={on}
          aria-disabled={busy || !status || undefined}
          aria-describedby="settings-apps-help"
          onClick={() => void flip()}
        />
      </div>
      <p className="hint" id="settings-apps-help">
        {t({
          id: 'mcp.help',
          message: 'AI agents and other MCP apps can edit the open project. Cuts always wait for your Apply.',
        })}
      </p>
      <p className="apps-status" role="status">
        {said}
      </p>
      {error && <FieldError id="settings-apps-error" text={error} />}
      <div className="input-row">
        <button
          type="button"
          className="btn btn-secondary"
          disabled={!status || status.needsMove}
          onClick={() => void copy()}
        >
          {t({ id: 'mcp.copy', message: 'Copy config' })}
        </button>
      </div>
      {status?.needsMove ? (
        <Note
          text={t({
            id: 'mcp.needsMove',
            message: 'Move Klipolo to your Applications folder first, then copy the config.',
          })}
        />
      ) : copied === 'done' ? (
        // Said here, not in a toast: the dialog's scrim would cover one.
        <p className="status-ok" role="status">
          <Icon name="check" />
          {t({
            id: 'mcp.copied',
            message: "Copied. Paste it into your app's MCP settings, then restart that app.",
          })}
        </p>
      ) : copied === 'failed' ? (
        <FieldError
          id="settings-apps-copy-error"
          text={t({ id: 'mcp.copyFailed', message: "Klipolo couldn't copy the config. Try again." })}
        />
      ) : null}
    </section>
  );
}
