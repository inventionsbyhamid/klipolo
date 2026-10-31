//! A 429 with no error code (Traefik's plain-text rate limit in front of the relay) is a busy moment the Meta adapter
//! waits out and sends again, like the relay's own `busy`; a used-up allowance stays a stop.

use klipolo_analysis::providers::ProviderErrorKind;
use klipolo_analysis::providers::meta::provider_error;
use klipolo_meta::MetaError;

fn limited(code: Option<&str>) -> MetaError {
    MetaError::RateLimited {
        retry_after: Some(2),
        code: code.map(str::to_owned),
        message: None,
    }
}

#[test]
fn a_429_with_no_error_code_is_busy_not_a_stop() {
    let busy = provider_error(limited(None), true);
    assert_eq!(busy.kind, ProviderErrorKind::Busy);
    assert!(!busy.is_stop());
    assert_eq!(busy.retry_after_s, Some(2));
    assert_eq!(
        busy.user_message(),
        "Too many requests right now. Try again in 2 seconds."
    );
}

#[test]
fn the_relays_busy_is_busy_and_a_quota_is_still_a_stop() {
    assert_eq!(
        provider_error(limited(Some("busy")), true).kind,
        ProviderErrorKind::Busy
    );
    for stop in ["align_quota", "rate_limit_exceeded"] {
        let error = provider_error(limited(Some(stop)), false);
        assert_eq!(error.kind, ProviderErrorKind::RateLimited, "{stop}");
        assert!(error.is_stop(), "{stop}");
    }
}
