use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserCategory {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub sort_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CategoryWithFeedCount {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub sort_order: i32,
    pub feed_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TagWithFeedCount {
    pub name: String,
    pub feed_count: i64,
}

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    parent_id: Option<Uuid>,
) -> Result<UserCategory> {
    let category = sqlx::query_as(
        "INSERT INTO user_categories (id, user_id, name, parent_id) \
         SELECT $1, $2, $3, $4 WHERE $4::uuid IS NULL OR EXISTS \
           (SELECT 1 FROM user_categories WHERE id = $4 AND user_id = $2) \
         ON CONFLICT (user_id, name) DO NOTHING \
         RETURNING id, name, parent_id, sort_order",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(name)
    .bind(parent_id)
    .fetch_optional(pool)
    .await?;
    match category {
        Some(category) => Ok(category),
        None => sqlx::query_as(
            "SELECT id, name, parent_id, sort_order FROM user_categories WHERE user_id = $1 AND name = $2",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound),
    }
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<UserCategory>> {
    Ok(sqlx::query_as(
        "SELECT id, name, parent_id, sort_order FROM user_categories WHERE user_id = $1 \
         ORDER BY sort_order, name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_with_feed_counts(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<CategoryWithFeedCount>> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE descendants(root_id, id) AS ( \
             SELECT id, id FROM user_categories WHERE user_id = $1 \
             UNION \
             SELECT descendants.root_id, child.id FROM descendants \
             JOIN user_categories child ON child.parent_id = descendants.id AND child.user_id = $1 \
         ) \
         SELECT c.id, c.name, c.parent_id, c.sort_order, count(DISTINCT uf.id)::bigint AS feed_count \
         FROM user_categories c \
         LEFT JOIN descendants ON descendants.root_id = c.id \
         LEFT JOIN user_feed_categories ufc ON ufc.user_category_id = descendants.id \
         LEFT JOIN user_feeds uf ON uf.id = ufc.user_feed_id AND uf.user_id = $1 \
         WHERE c.user_id = $1 \
         GROUP BY c.id ORDER BY c.sort_order, c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

pub async fn belongs_to_user(pool: &PgPool, user_id: Uuid, category_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM user_categories WHERE id = $1 AND user_id = $2",
        )
        .bind(category_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .is_some(),
    )
}

pub async fn list_tags(pool: &PgPool, user_id: Uuid) -> Result<Vec<TagWithFeedCount>> {
    Ok(sqlx::query_as(
        "SELECT tag.name, count(DISTINCT uf.id)::bigint AS feed_count \
         FROM user_feeds uf CROSS JOIN LATERAL unnest(uf.tags) AS tag(name) \
         WHERE uf.user_id = $1 AND tag.name <> '' GROUP BY tag.name ORDER BY tag.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}
