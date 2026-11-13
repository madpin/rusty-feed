//! Shared story clusters. Embeddings are compared once; membership is not
//! copied per subscriber.

use sqlx::PgPool;
use uuid::Uuid;

use rusty_feed_core::reading::{joins_story_cluster, BriefingSource, BriefingStory};

use crate::error::Result;

const BATCH_LIMIT: i64 = 40;

/// How many articles a clustering pass attached or opened as new stories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClusterStats {
    pub considered: u64,
    pub joined: u64,
    pub created: u64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Unclustered {
    id: Uuid,
    title: String,
    content_revision: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct Neighbor {
    article_id: Uuid,
    content_revision: i64,
    cluster_id: Option<Uuid>,
    similarity: f64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SummaryCandidate {
    id: Uuid,
    title: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MemberTitle {
    title: String,
    excerpt: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CoverageRow {
    summary: Option<String>,
    article_id: Uuid,
    title: String,
    feed_name: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DigestRow {
    cluster_id: Uuid,
    headline: String,
    summary: Option<String>,
    article_id: Uuid,
    title: String,
    feed_name: String,
}

/// Other subscribed articles in the same shared story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleCoverage {
    pub summary: Option<String>,
    pub sources: Vec<CoverageSource>,
}

/// One other article the current user may open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSource {
    pub article_id: Uuid,
    pub title: String,
    pub feed_name: String,
}

/// Drop membership that no longer matches the embedded revision, then attach a
/// bounded batch of recent embedded articles to an existing story or to one
/// other unmatched article. A lone article does not become a story.
pub async fn cluster_recent(pool: &PgPool) -> Result<ClusterStats> {
    sqlx::query(
        "DELETE FROM story_cluster_members AS member \
         USING articles AS article \
         WHERE article.id = member.article_id \
           AND article.content_revision <> member.content_revision",
    )
    .execute(pool)
    .await?;
    // A single article is not a story. Drop leftovers from an earlier pass that
    // opened a cluster for every embedded article, and any cluster whose members
    // were just removed. Membership rows cascade with the cluster.
    sqlx::query(
        "DELETE FROM story_clusters AS cluster \
         WHERE ( \
             SELECT count(*) FROM story_cluster_members AS member \
             WHERE member.cluster_id = cluster.id \
         ) < 2",
    )
    .execute(pool)
    .await?;

    let Some(model) = active_model(pool).await? else {
        return Ok(ClusterStats::default());
    };
    let pending: Vec<Unclustered> = sqlx::query_as(
        "SELECT article.id, article.title, article.content_revision \
         FROM articles AS article \
         JOIN article_embeddings AS embedding ON embedding.article_id = article.id \
           AND embedding.model = $1 \
           AND embedding.content_revision = article.content_revision \
         LEFT JOIN story_cluster_members AS member \
           ON member.article_id = article.id \
          AND member.content_revision = article.content_revision \
         WHERE member.article_id IS NULL \
           AND coalesce(article.published_at, article.created_at) > now() - interval '14 days' \
         ORDER BY coalesce(article.published_at, article.created_at) DESC \
         LIMIT $2",
    )
    .bind(&model)
    .bind(BATCH_LIMIT)
    .fetch_all(pool)
    .await?;

    let mut stats = ClusterStats {
        considered: pending.len() as u64,
        ..ClusterStats::default()
    };
    for article in pending {
        match attach_article(pool, &model, &article).await? {
            Attach::Joined => stats.joined += 1,
            Attach::Created => stats.created += 1,
            Attach::Skipped => {}
        }
    }
    Ok(stats)
}

enum Attach {
    Joined,
    Created,
    Skipped,
}

/// Place one embedded article into a shared story. Returns the cluster id.
pub async fn cluster_article(pool: &PgPool, article_id: Uuid) -> Result<Option<Uuid>> {
    let Some(model) = model_for_article(pool, article_id).await? else {
        return Ok(None);
    };
    let Some(article) = sqlx::query_as::<_, Unclustered>(
        "SELECT article.id, article.title, article.content_revision \
         FROM articles AS article \
         JOIN article_embeddings AS embedding ON embedding.article_id = article.id \
           AND embedding.model = $2 \
           AND embedding.content_revision = article.content_revision \
         WHERE article.id = $1",
    )
    .bind(article_id)
    .bind(&model)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    attach_article(pool, &model, &article).await?;
    Ok(
        sqlx::query_scalar("SELECT cluster_id FROM story_cluster_members WHERE article_id = $1")
            .bind(article_id)
            .fetch_optional(pool)
            .await?,
    )
}

async fn attach_article(pool: &PgPool, model: &str, article: &Unclustered) -> Result<Attach> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT cluster_id FROM story_cluster_members \
         WHERE article_id = $1 AND content_revision = $2",
    )
    .bind(article.id)
    .bind(article.content_revision)
    .fetch_optional(pool)
    .await?;
    if existing.is_some() {
        return Ok(Attach::Skipped);
    }
    let nearest: Option<Neighbor> = sqlx::query_as(
        "SELECT other.article_id, article.content_revision, member.cluster_id, \
                1 - ((other.embedding <=> mine.embedding) / 2) AS similarity \
         FROM article_embeddings AS mine \
         JOIN article_embeddings AS other \
           ON other.model = mine.model AND other.article_id <> mine.article_id \
         JOIN articles AS article ON article.id = other.article_id \
           AND article.content_revision = other.content_revision \
         LEFT JOIN story_cluster_members AS member ON member.article_id = other.article_id \
         WHERE mine.article_id = $1 AND mine.model = $2 \
           AND coalesce(article.published_at, article.created_at) > now() - interval '14 days' \
         ORDER BY other.embedding <=> mine.embedding \
         LIMIT 1",
    )
    .bind(article.id)
    .bind(model)
    .fetch_optional(pool)
    .await?;
    let Some(nearest) = nearest.filter(|hit| joins_story_cluster(hit.similarity)) else {
        return Ok(Attach::Skipped);
    };
    if let Some(cluster_id) = nearest.cluster_id {
        return join_cluster(pool, article, cluster_id, nearest.similarity).await;
    }
    pair_unclustered(pool, model, article, &nearest).await
}

async fn join_cluster(
    pool: &PgPool,
    article: &Unclustered,
    cluster_id: Uuid,
    similarity: f64,
) -> Result<Attach> {
    let inserted = insert_member(
        pool,
        article.id,
        cluster_id,
        article.content_revision,
        similarity,
    )
    .await?;
    if inserted == 0 {
        return Ok(Attach::Skipped);
    }
    refresh_count(pool, cluster_id).await?;
    Ok(Attach::Joined)
}

/// Open one shared story for two articles that are not in a story yet.
///
/// If the neighbor is claimed by another pass before this insert commits, this
/// article joins that story instead of leaving a one-article cluster behind.
async fn pair_unclustered(
    pool: &PgPool,
    model: &str,
    article: &Unclustered,
    neighbor: &Neighbor,
) -> Result<Attach> {
    let cluster_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let taken: Option<Uuid> =
        sqlx::query_scalar("SELECT cluster_id FROM story_cluster_members WHERE article_id = $1")
            .bind(neighbor.article_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(existing) = taken {
        let inserted = insert_member_tx(
            &mut tx,
            article.id,
            existing,
            article.content_revision,
            neighbor.similarity,
        )
        .await?;
        if inserted > 0 {
            refresh_count_tx(&mut tx, existing).await?;
        }
        tx.commit().await?;
        return Ok(if inserted > 0 {
            Attach::Joined
        } else {
            Attach::Skipped
        });
    }
    sqlx::query(
        "INSERT INTO story_clusters \
            (id, title, representative_article_id, member_count, model) \
         VALUES ($1, $2, $3, 2, $4)",
    )
    .bind(cluster_id)
    .bind(&article.title)
    .bind(article.id)
    .bind(model)
    .execute(&mut *tx)
    .await?;
    let inserted = insert_member_tx(
        &mut tx,
        article.id,
        cluster_id,
        article.content_revision,
        neighbor.similarity,
    )
    .await?;
    if inserted == 0 {
        tx.rollback().await?;
        return Ok(Attach::Skipped);
    }
    let neighbor_inserted = insert_member_tx(
        &mut tx,
        neighbor.article_id,
        cluster_id,
        neighbor.content_revision,
        neighbor.similarity,
    )
    .await?;
    if neighbor_inserted == 0 {
        sqlx::query("DELETE FROM story_clusters WHERE id = $1")
            .bind(cluster_id)
            .execute(&mut *tx)
            .await?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT cluster_id FROM story_cluster_members WHERE article_id = $1",
        )
        .bind(neighbor.article_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing) = existing {
            let joined = insert_member_tx(
                &mut tx,
                article.id,
                existing,
                article.content_revision,
                neighbor.similarity,
            )
            .await?;
            if joined > 0 {
                refresh_count_tx(&mut tx, existing).await?;
            }
            tx.commit().await?;
            return Ok(if joined > 0 {
                Attach::Joined
            } else {
                Attach::Skipped
            });
        }
        tx.commit().await?;
        return Ok(Attach::Skipped);
    }
    tx.commit().await?;
    Ok(Attach::Created)
}

async fn insert_member(
    pool: &PgPool,
    article_id: Uuid,
    cluster_id: Uuid,
    content_revision: i64,
    similarity: f64,
) -> Result<u64> {
    Ok(sqlx::query(
        "INSERT INTO story_cluster_members \
            (article_id, cluster_id, content_revision, similarity) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (article_id) DO NOTHING",
    )
    .bind(article_id)
    .bind(cluster_id)
    .bind(content_revision)
    .bind(similarity as f32)
    .execute(pool)
    .await?
    .rows_affected())
}

async fn insert_member_tx(
    tx: &mut sqlx::PgConnection,
    article_id: Uuid,
    cluster_id: Uuid,
    content_revision: i64,
    similarity: f64,
) -> Result<u64> {
    Ok(sqlx::query(
        "INSERT INTO story_cluster_members \
            (article_id, cluster_id, content_revision, similarity) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (article_id) DO NOTHING",
    )
    .bind(article_id)
    .bind(cluster_id)
    .bind(content_revision)
    .bind(similarity as f32)
    .execute(&mut *tx)
    .await?
    .rows_affected())
}

/// Stories that have several sources and still need a shared summary.
pub async fn clusters_needing_summary(pool: &PgPool, limit: i64) -> Result<Vec<(Uuid, String)>> {
    let rows: Vec<SummaryCandidate> = sqlx::query_as(
        "SELECT id, title FROM story_clusters \
         WHERE summary IS NULL AND member_count >= 2 \
         ORDER BY updated_at DESC \
         LIMIT $1",
    )
    .bind(limit.clamp(1, 20))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| (row.id, row.title)).collect())
}

/// Titles and short excerpts for a shared summary prompt.
pub async fn summary_sources(pool: &PgPool, cluster_id: Uuid) -> Result<Vec<(String, String)>> {
    let rows: Vec<MemberTitle> = sqlx::query_as(
        "SELECT article.title, article.excerpt \
         FROM story_cluster_members AS member \
         JOIN articles AS article ON article.id = member.article_id \
         WHERE member.cluster_id = $1 \
         ORDER BY coalesce(article.published_at, article.created_at) DESC \
         LIMIT 6",
    )
    .bind(cluster_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.title, row.excerpt.unwrap_or_default()))
        .collect())
}

/// Store one shared story summary. Later writers do not replace an existing one.
pub async fn set_summary(pool: &PgPool, cluster_id: Uuid, summary: &str) -> Result<bool> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Ok(false);
    }
    let updated = sqlx::query(
        "UPDATE story_clusters SET summary = $2, summarized_at = now(), updated_at = now() \
         WHERE id = $1 AND summary IS NULL",
    )
    .bind(cluster_id)
    .bind(summary)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(updated > 0)
}

/// Coverage the user can open from one article, excluding the article itself.
pub async fn coverage_for_user(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
) -> Result<ArticleCoverage> {
    let rows: Vec<CoverageRow> = sqlx::query_as(
        "SELECT cluster.summary, article.id AS article_id, article.title, \
                coalesce(nullif(subscription.custom_name, ''), feed.name) AS feed_name \
         FROM story_cluster_members AS mine \
         JOIN story_clusters AS cluster ON cluster.id = mine.cluster_id \
         JOIN story_cluster_members AS other \
           ON other.cluster_id = mine.cluster_id AND other.article_id <> mine.article_id \
         JOIN articles AS article ON article.id = other.article_id \
         JOIN feeds AS feed ON feed.id = article.feed_id \
         JOIN user_feeds AS subscription \
           ON subscription.feed_id = article.feed_id AND subscription.user_id = $1 \
         WHERE mine.article_id = $2 \
         ORDER BY coalesce(article.published_at, article.created_at) DESC \
         LIMIT 8",
    )
    .bind(user_id)
    .bind(article_id)
    .fetch_all(pool)
    .await?;
    let summary = rows.first().and_then(|row| row.summary.clone());
    let sources = rows
        .into_iter()
        .map(|row| CoverageSource {
            article_id: row.article_id,
            title: row.title,
            feed_name: row.feed_name,
        })
        .collect();
    Ok(ArticleCoverage { summary, sources })
}

/// How many of one reader's recent articles exist and how many have a current
/// search-index entry. Stories can only form among embedded articles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct EmbeddingCoverage {
    pub recent: i64,
    pub embedded: i64,
}

/// Count articles from the user's feeds inside the story window (14 days) and
/// how many of them are embedded at their current revision.
pub async fn embedding_coverage_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<EmbeddingCoverage> {
    Ok(sqlx::query_as(
        "SELECT count(*) AS recent, count(embedding.article_id) AS embedded \
         FROM articles AS article \
         JOIN user_feeds AS subscription \
           ON subscription.feed_id = article.feed_id AND subscription.user_id = $1 \
         LEFT JOIN article_embeddings AS embedding ON embedding.article_id = article.id \
          AND embedding.content_revision = article.content_revision \
         WHERE coalesce(article.published_at, article.created_at) > now() - interval '14 days'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

/// Recent multi-source stories visible to one user, for a personal briefing.
pub async fn briefing_stories(pool: &PgPool, user_id: Uuid) -> Result<Vec<BriefingStory>> {
    let rows: Vec<DigestRow> = sqlx::query_as(
        "SELECT cluster.id AS cluster_id, cluster.title AS headline, cluster.summary, \
                article.id AS article_id, article.title, \
                coalesce(nullif(subscription.custom_name, ''), feed.name) AS feed_name \
         FROM story_clusters AS cluster \
         JOIN story_cluster_members AS member ON member.cluster_id = cluster.id \
         JOIN articles AS article ON article.id = member.article_id \
         JOIN feeds AS feed ON feed.id = article.feed_id \
         JOIN user_feeds AS subscription \
           ON subscription.feed_id = article.feed_id AND subscription.user_id = $1 \
         WHERE cluster.member_count >= 2 \
           AND cluster.updated_at > now() - interval '36 hours' \
         ORDER BY cluster.member_count DESC, cluster.updated_at DESC, \
                  coalesce(article.published_at, article.created_at) DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut grouped: Vec<(Uuid, BriefingStory)> = Vec::new();
    for row in rows {
        let href = format!("/articles/{}", row.article_id);
        let source = BriefingSource {
            title: row.title,
            href,
            feed_name: row.feed_name,
        };
        if let Some((_, story)) = grouped.iter_mut().find(|(id, _)| *id == row.cluster_id) {
            if story.sources.len() < 6
                && story
                    .sources
                    .iter()
                    .all(|existing| existing.href != source.href)
            {
                story.sources.push(source);
            }
            continue;
        }
        if grouped.len() >= 12 {
            continue;
        }
        grouped.push((
            row.cluster_id,
            BriefingStory {
                headline: row.headline,
                summary: row.summary.unwrap_or_default(),
                sources: vec![source],
            },
        ));
    }
    Ok(grouped.into_iter().map(|(_, story)| story).collect())
}

async fn active_model(pool: &PgPool) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT model FROM instance_embedding_space WHERE singleton")
            .fetch_optional(pool)
            .await?,
    )
}

/// Instance model when one is configured; otherwise the model already stored
/// on this article. Never picks an unrelated model from another row.
async fn model_for_article(pool: &PgPool, article_id: Uuid) -> Result<Option<String>> {
    if let Some(model) = active_model(pool).await? {
        return Ok(Some(model));
    }
    Ok(
        sqlx::query_scalar("SELECT model FROM article_embeddings WHERE article_id = $1")
            .bind(article_id)
            .fetch_optional(pool)
            .await?,
    )
}

async fn refresh_count(pool: &PgPool, cluster_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE story_clusters SET \
            member_count = (SELECT count(*) FROM story_cluster_members WHERE cluster_id = $1), \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(cluster_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn refresh_count_tx(tx: &mut sqlx::PgConnection, cluster_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE story_clusters SET \
            member_count = (SELECT count(*) FROM story_cluster_members WHERE cluster_id = $1), \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(cluster_id)
    .execute(&mut *tx)
    .await?;
    Ok(())
}
