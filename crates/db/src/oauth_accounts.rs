//! OAuth account repository — links a third-party identity to a user.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{OAuthAccount, OAUTH_ACCOUNT_COLUMNS};

/// Find a linked account by provider slug and the provider's account id.
pub async fn find(
    pool: &PgPool,
    provider: &str,
    provider_account_id: &str,
) -> Result<Option<OAuthAccount>> {
    let sql = format!(
        "SELECT {OAUTH_ACCOUNT_COLUMNS} FROM oauth_accounts \
         WHERE provider = $1 AND provider_account_id = $2"
    );
    Ok(sqlx::query_as::<_, OAuthAccount>(&sql)
        .bind(provider)
        .bind(provider_account_id)
        .fetch_optional(pool)
        .await?)
}

/// Link an OAuth identity to a user.
pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    provider: &str,
    provider_account_id: &str,
) -> Result<OAuthAccount> {
    let sql = format!(
        "INSERT INTO oauth_accounts (id, user_id, provider, provider_account_id) \
         VALUES ($1, $2, $3, $4) RETURNING {OAUTH_ACCOUNT_COLUMNS}"
    );
    Ok(sqlx::query_as::<_, OAuthAccount>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(provider)
        .bind(provider_account_id)
        .fetch_one(pool)
        .await?)
}
