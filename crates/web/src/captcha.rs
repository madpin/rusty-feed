//! Server-side CAPTCHA verification (Cloudflare Turnstile / hCaptcha) for
//! registration anti-abuse (`docs/PUBLIC_LAUNCH_PLAN.md` §4.5). Verified via the
//! SSRF-guarded fetch client; the secret is never rendered to the browser.

use serde::Deserialize;

use crate::error::AppError;
use crate::state::AppState;

const TURNSTILE_VERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const HCAPTCHA_VERIFY_URL: &str = "https://hcaptcha.com/siteverify";

#[derive(Deserialize)]
struct SiteVerify {
    #[serde(default)]
    success: bool,
}

/// Verify a CAPTCHA `token` from the client. Returns `Ok(())` when the CAPTCHA
/// is disabled or the token is valid; `Err(Validation)` otherwise. A network
/// failure fails **closed** (rejects), since CAPTCHA gates abuse.
pub async fn verify(state: &AppState, token: &str, remote_ip: &str) -> Result<(), AppError> {
    let Some(settings) = &state.settings.captcha else {
        return Ok(());
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::Validation(
            "Please complete the anti-bot challenge.".to_string(),
        ));
    }
    let url = match settings.provider.as_str() {
        "hcaptcha" => HCAPTCHA_VERIFY_URL,
        _ => TURNSTILE_VERIFY_URL,
    };
    let mut params = vec![("secret", settings.secret.as_str()), ("response", token)];
    if !remote_ip.is_empty() && remote_ip != "unknown" {
        params.push(("remoteip", remote_ip));
    }
    let response = state
        .fetch
        .client()
        .post(url)
        .form(&params)
        .send()
        .await
        .map_err(|error| {
            tracing::warn!(error = %error, "CAPTCHA verification request failed");
            AppError::Validation(
                "The anti-bot challenge could not be verified. Please try again.".to_string(),
            )
        })?;
    let outcome: SiteVerify = response.json().await.map_err(|error| {
        tracing::warn!(error = %error, "CAPTCHA verification response was unreadable");
        AppError::Validation(
            "The anti-bot challenge could not be verified. Please try again.".to_string(),
        )
    })?;
    if outcome.success {
        Ok(())
    } else {
        Err(AppError::Validation(
            "The anti-bot challenge failed. Please try again.".to_string(),
        ))
    }
}
