//! The recorder's permission steps, each only on the person's own press in the Record dialog (a permission request
//! begins only after an explicit press; none reaches chat or MCP):
//!
//! - **Allow screen recording** asks macOS for Screen Recording (macOS lists Klipolo and asks the first time), then
//!   opens that settings page, so the person can turn Klipolo on there.
//! - **Reopen Klipolo** saves the open project and starts Klipolo again, as macOS applies a Screen Recording grant
//!   only to a relaunched app. Refused while a take is armed through saving (`capture.busy`) and while an export runs
//!   (the restart would end it); a save that fails keeps Klipolo open and says why. From its start no take arms. As it
//!   goes ahead it is remembered (`screen_reopen`), so the step can say when macOS still refuses.
//! - **Open privacy settings** opens Screen Recording's, the camera's or the microphone's page. It only opens the page.

use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_capture::backend::Backend;
use klipolo_model::coded::{CodedError, Said};

use super::screen_reopen;
use super::setup::Files;
use super::take::Recorder;
use super::types::PrivacyPage;
use super::words;

/// macOS's privacy settings page for `page`.
pub(super) fn privacy_url(page: PrivacyPage) -> &'static str {
    match page {
        PrivacyPage::Screen => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PrivacyPage::Camera => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
        }
        PrivacyPage::Mic => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
    }
}

/// How a settings page is opened: macOS's `open` (`open_url`), or a test's record.
pub(super) type Open<'a> = &'a dyn Fn(&str) -> std::io::Result<()>;

/// "Allow screen recording": asks `backend` for Screen Recording, then opens its page with `open`, in that order:
/// macOS lists Klipolo there only once it has asked.
pub(super) fn allow_screen(backend: &dyn Backend, open: Open<'_>) {
    backend.request_screen_permission();
    if let Err(error) = open(privacy_url(PrivacyPage::Screen)) {
        tracing::warn!(%error, "recorder: Screen Recording's settings didn't open");
    }
}

/// "Open privacy settings": `page` with `open`; `capture.permission.settingsDidntOpen` when it didn't open.
pub(super) fn open_privacy(page: PrivacyPage, open: Open<'_>) -> Result<(), CodedError> {
    open(privacy_url(page)).map_err(|e| words::settings_didnt_open(&e))
}

/// "Reopen Klipolo": refused while an export runs (`exporting`: `capture.permission.reopenExporting`, asked again once
/// the project is saved, as one may start meanwhile) or `recorder` has a take, armed through saving (`capture.busy`).
/// Otherwise no take arms from now on, the project is saved (`flush`; a failure keeps Klipolo open and says why, and
/// takes arm again), the reopen is remembered in `files`, and Klipolo starts again (`restart`). A second press while it
/// is under way does nothing more.
pub(super) fn reopen(
    recorder: &Recorder,
    files: Option<&Files>,
    exporting: &dyn Fn() -> bool,
    flush: impl FnOnce() -> Result<(), Said>,
    restart: impl FnOnce(),
) -> Result<(), Said> {
    if exporting() {
        return Err(Said::Coded(words::reopen_exporting()));
    }
    if !recorder.begin_reopen()? {
        tracing::info!("recorder: Reopen Klipolo is under way already");
        return Ok(());
    }
    let saved = flush().and_then(|()| {
        if exporting() {
            return Err(Said::Coded(words::reopen_exporting()));
        }
        Ok(())
    });
    if let Err(said) = saved {
        recorder.end_reopen();
        return Err(said);
    }
    if let Some(files) = files {
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        if let Err(error) = screen_reopen::remember(files, at_ms) {
            tracing::warn!(%error, "recorder: Reopen Klipolo wasn't remembered");
        }
    }
    tracing::info!("recorder: Klipolo reopens for Screen Recording");
    restart();
    Ok(())
}

/// Opens `url` with macOS's `open`.
#[cfg(target_os = "macos")]
pub(super) fn open_url(url: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("open").arg(url).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("open ended with {status}")))
    }
}

/// Elsewhere there are no such pages.
#[cfg(not(target_os = "macos"))]
pub(super) fn open_url(_url: &str) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}
