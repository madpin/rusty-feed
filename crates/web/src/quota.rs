//! Per-user resource quota enforcement (FND-188). Call [`assert_within`] at each
//! per-user create path; it returns `AppError::QuotaExceeded` (409) when the cap
//! is reached. Quotas count per-user rows only and never touch shared content.

use rustss_core::quotas::QuotaKind;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Reject creating another `kind` for `user_id` when the configured cap is hit.
/// Unlimited (`0`) short-circuits without a query.
pub async fn assert_within(
    state: &AppState,
    user_id: Uuid,
    kind: QuotaKind,
) -> Result<(), AppError> {
    let quotas = &state.settings.quotas;
    if quotas.limit(kind) == 0 {
        return Ok(());
    }
    let pool = state.db.pool();
    let count = match kind {
        QuotaKind::Subscriptions => rustss_db::subscriptions::count_for_user(pool, user_id).await?,
        QuotaKind::SavedSearches => {
            rustss_db::saved_searches::count_for_user(pool, user_id).await?
        }
        QuotaKind::Categories => rustss_db::categories::count_for_user(pool, user_id).await?,
        QuotaKind::ApiTokens => rustss_db::api_tokens::count_for_user(pool, user_id).await?,
        QuotaKind::Integrations => rustss_db::integrations::count_for_user(pool, user_id).await?,
        QuotaKind::PublicShares => rustss_db::public_shares::count_for_user(pool, user_id).await?,
    };
    if quotas.allows(kind, count) {
        Ok(())
    } else {
        Err(AppError::QuotaExceeded(quotas.exceeded_message(kind)))
    }
}
