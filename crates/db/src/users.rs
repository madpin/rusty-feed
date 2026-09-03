//! User repository.

use sqlx::PgPool;
use uuid::Uuid;

use rusty_feed_core::feed_content::{
    CATALOG_ARTICLE_PAGE_RECOMMENDATIONS, DEFAULT_CONTENT_LIFETIME_DAYS,
};

use crate::error::Result;
use crate::models::User;

const USER_COLUMNS: &str =
    "id, email, name, image, role, password_hash, email_verified_at, created_at, updated_at, \
     last_seen_at, failed_login_count, locked_until, tos_accepted_at, is_demo, onboarded_at";

/// Create a user. `role` must be one of `admin` | `user` | `guest`.
pub async fn create(
    pool: &PgPool,
    email: &str,
    name: Option<&str>,
    image: Option<&str>,
    role: &str,
    password_hash: Option<&str>,
) -> Result<User> {
    let sql = format!(
        "INSERT INTO users (id, email, name, image, role, password_hash) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(image)
        .bind(role)
        .bind(password_hash)
        .fetch_one(pool)
        .await?;
    Ok(user)
}

/// Create an ephemeral demo account (§7): flagged `is_demo`, no password, and
/// seeded with the featured catalog organized into personal folders/tags by
/// catalog category. Featured feeds are demo showcase content only; regular
/// accounts start empty and choose feeds in the wizard/catalog. `role` is
/// `user` or `admin_readonly` (never a full admin).
pub async fn create_demo(pool: &PgPool, email: &str, name: &str, role: &str) -> Result<User> {
    let mut transaction = pool.begin().await?;
    let sql = format!(
        "INSERT INTO users (id, email, name, role, is_demo, email_verified_at) \
         VALUES ($1, $2, $3, $4, true, now()) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(role)
        .fetch_one(&mut *transaction)
        .await?;
    seed_demo_catalog(&mut transaction, user.id).await?;
    sqlx::query(
        "INSERT INTO user_preferences (user_id, preferences) \
         VALUES ($1, '{\"theme\":\"rusty\"}'::jsonb) \
         ON CONFLICT (user_id) DO UPDATE SET \
           preferences = user_preferences.preferences || EXCLUDED.preferences, \
           updated_at = now()",
    )
    .bind(user.id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(user)
}

/// Delete every demo account (§7 daily reset). Cascades all per-user rows and
/// clears non-cascading `cost_ledger` residue first (that FK is SET NULL).
/// Scoped strictly to `is_demo = true` so real users are never touched.
pub async fn delete_all_demo(pool: &PgPool) -> Result<u64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM cost_ledger WHERE user_id IN (SELECT id FROM users WHERE is_demo = true)",
    )
    .execute(&mut *tx)
    .await?;
    let deleted = sqlx::query("DELETE FROM users WHERE is_demo = true")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}

/// Atomically consume an invitation and create its user. Returns `None` when
/// the token is expired, revoked, already used, or restricted to a different
/// email address. Regular invited accounts start without subscriptions.
pub async fn create_with_invitation(
    pool: &PgPool,
    invitation_hash: &str,
    email: &str,
    name: Option<&str>,
    password_hash: &str,
) -> Result<Option<User>> {
    let mut transaction = pool.begin().await?;
    let invitation_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM registration_invitations \
         WHERE token_hash = $1 AND revoked_at IS NULL AND accepted_at IS NULL \
           AND expires_at > now() AND (email IS NULL OR lower(email) = lower($2)) \
         FOR UPDATE",
    )
    .bind(invitation_hash)
    .bind(email)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(invitation_id) = invitation_id else {
        return Ok(None);
    };

    let sql = format!(
        "INSERT INTO users (id, email, name, role, password_hash) \
         VALUES ($1, $2, $3, 'user', $4) RETURNING {USER_COLUMNS}"
    );
    let user = sqlx::query_as::<_, User>(&sql)
        .bind(Uuid::now_v7())
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE registration_invitations SET accepted_by = $2, accepted_at = now() WHERE id = $1",
    )
    .bind(invitation_id)
    .bind(user.id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(user))
}

/// Seed one new demo account with every published featured feed, then mirror
/// each feed's catalog category into a personal folder and tag. All writes use
/// the account-creation transaction so a demo never becomes visible half-seeded.
async fn seed_demo_catalog(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_feeds (id, user_id, feed_id, settings) \
         SELECT gen_random_uuid(), $1, c.feed_id, CASE WHEN f.url = ANY($2) \
           THEN jsonb_build_object('content_mode', 'article_page', 'content_lifetime_days', $3::int) \
           ELSE NULL END \
         FROM feed_catalog_entries c JOIN feeds f ON f.id = c.feed_id \
         WHERE c.published AND c.featured ORDER BY c.sort_order, c.title \
         ON CONFLICT (user_id, feed_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(CATALOG_ARTICLE_PAGE_RECOMMENDATIONS)
    .bind(i32::from(DEFAULT_CONTENT_LIFETIME_DAYS))
    .execute(&mut **transaction)
    .await?;

    sqlx::query(
        "INSERT INTO user_categories (id, user_id, name, parent_id) \
         SELECT gen_random_uuid(), $1, category, NULL FROM ( \
           SELECT DISTINCT left(trim(category), 40) AS category \
           FROM feed_catalog_entries WHERE published AND featured \
         ) categories WHERE category <> '' \
         ON CONFLICT (user_id, parent_id, name) DO NOTHING",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;

    sqlx::query(
        "UPDATE user_feeds uf SET tags = ARRAY[left(trim(c.category), 40)], updated_at = now() \
         FROM feed_catalog_entries c \
         WHERE uf.user_id = $1 AND uf.feed_id = c.feed_id \
           AND c.published AND c.featured AND trim(c.category) <> ''",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;

    sqlx::query(
        "INSERT INTO user_feed_categories (id, user_feed_id, user_category_id) \
         SELECT gen_random_uuid(), uf.id, category.id \
         FROM user_feeds uf \
         JOIN feed_catalog_entries c ON c.feed_id = uf.feed_id AND c.published AND c.featured \
         JOIN user_categories category \
           ON category.user_id = uf.user_id AND category.name = left(trim(c.category), 40) \
         WHERE uf.user_id = $1 AND trim(c.category) <> '' \
         ON CONFLICT (user_feed_id, user_category_id) DO NOTHING",
    )
    .bind(user_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Look up a user by email.
pub async fn find_by_email(pool: &PgPool, email: &str) -> Result<Option<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE email = $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(email)
        .fetch_optional(pool)
        .await?)
}

/// Look up a user by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Total number of users (used for the "first user becomes admin" bootstrap).
pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await?)
}

pub async fn list(pool: &PgPool, limit: i64) -> Result<Vec<User>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users ORDER BY created_at DESC LIMIT $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Record that a user was just seen, throttled to at most once per minute per
/// user so busy sessions do not write on every request. Best-effort.
pub async fn touch_last_seen(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET last_seen_at = now() \
         WHERE id = $1 AND (last_seen_at IS NULL OR last_seen_at < now() - interval '1 minute')",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Most recently created accounts, newest first (admin activity view, §10.1).
pub async fn recent_signups(pool: &PgPool, limit: i64) -> Result<Vec<User>> {
    let limit = limit.clamp(1, 100);
    let sql = format!("SELECT {USER_COLUMNS} FROM users ORDER BY created_at DESC LIMIT $1");
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Accounts currently locked out from local login (admin activity view, §10.1).
pub async fn locked_accounts(pool: &PgPool, limit: i64) -> Result<Vec<User>> {
    let limit = limit.clamp(1, 100);
    let sql = format!(
        "SELECT {USER_COLUMNS} FROM users WHERE locked_until IS NOT NULL AND locked_until > now() \
         ORDER BY locked_until DESC LIMIT $1"
    );
    Ok(sqlx::query_as::<_, User>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Stamp the first-run wizard as done (finished or skipped), idempotently (§8.4).
pub async fn mark_onboarded(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE users SET onboarded_at = COALESCE(onboarded_at, now()) WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-open the first-run wizard for this user ("Re-run setup").
pub async fn clear_onboarded(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE users SET onboarded_at = NULL WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Record consent to the Terms/Privacy Policy at registration (idempotent).
pub async fn record_consent(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET tos_accepted_at = COALESCE(tos_accepted_at, now()) WHERE id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Self-serve account deletion (GDPR/CCPA). Cascades all per-user rows. Guarded
/// against removing the last administrator so an instance is never orphaned.
/// Returns `Ok(false)` when blocked as the last admin.
pub async fn delete(pool: &PgPool, user_id: Uuid) -> Result<bool> {
    let mut tx = pool.begin().await?;
    // Advisory lock keyed on the admin-role space (same key change_role uses is
    // not required here; a simple guard suffices) — recount inside the tx.
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(false);
    };
    if role == "admin" {
        let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
            .fetch_one(&mut *tx)
            .await?;
        if admins <= 1 {
            tx.rollback().await?;
            return Ok(false);
        }
    }
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Mark an account's email address as verified (idempotent).
pub async fn mark_email_verified(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()), updated_at = now() \
         WHERE id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed local login. When `max_failures > 0` and the running count
/// reaches it, lock the account for `lockout_seconds` and reset the counter so
/// the next window starts fresh (FND-191).
pub async fn record_failed_login(
    pool: &PgPool,
    user_id: Uuid,
    max_failures: i32,
    lockout_seconds: i64,
) -> Result<()> {
    if max_failures <= 0 {
        return Ok(());
    }
    sqlx::query(
        "UPDATE users SET \
           failed_login_count = CASE WHEN failed_login_count + 1 >= $2 THEN 0 ELSE failed_login_count + 1 END, \
           locked_until = CASE WHEN failed_login_count + 1 >= $2 THEN now() + make_interval(secs => $3) ELSE locked_until END, \
           updated_at = now() \
         WHERE id = $1",
    )
    .bind(user_id)
    .bind(max_failures)
    .bind(lockout_seconds as f64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the failure counter and any lock after a successful login.
pub async fn clear_failed_logins(pool: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE users SET failed_login_count = 0, locked_until = NULL, updated_at = now() \
         WHERE id = $1 AND (failed_login_count <> 0 OR locked_until IS NOT NULL)",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn count_admins(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
            .fetch_one(pool)
            .await?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleChange {
    Changed,
    TargetNotFound,
    InvalidRole,
    CannotDemoteSelf,
    LastAdmin,
}

pub async fn change_role(
    pool: &PgPool,
    actor_id: Uuid,
    target_id: Uuid,
    role: &str,
) -> Result<RoleChange> {
    if !matches!(role, "admin" | "admin_readonly" | "user") {
        return Ok(RoleChange::InvalidRole);
    }
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(415943191::bigint)")
        .execute(&mut *transaction)
        .await?;
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1 FOR UPDATE");
    let target = sqlx::query_as::<_, User>(&sql)
        .bind(target_id)
        .fetch_optional(&mut *transaction)
        .await?;
    let Some(target) = target else {
        return Ok(RoleChange::TargetNotFound);
    };
    if target.id == actor_id && target.role == "admin" && role != "admin" {
        return Ok(RoleChange::CannotDemoteSelf);
    }
    if target.role == "admin" && role != "admin" {
        let admin_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin'")
                .fetch_one(&mut *transaction)
                .await?;
        if admin_count <= 1 {
            return Ok(RoleChange::LastAdmin);
        }
    }
    sqlx::query("UPDATE users SET role = $2, updated_at = now() WHERE id = $1")
        .bind(target_id)
        .bind(role)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(RoleChange::Changed)
}

/// Change a user's role.
pub async fn set_role(pool: &PgPool, id: Uuid, role: &str) -> Result<()> {
    sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
        .bind(id)
        .bind(role)
        .execute(pool)
        .await?;
    Ok(())
}
