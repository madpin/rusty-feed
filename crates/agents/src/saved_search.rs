//! Saved-search matching (M11).
//!
//! `SavedSearchScanAgent` fans out one deduped `saved_search_match` job per
//! active saved search; `SavedSearchMatchAgent` matches a recent candidate
//! window against that search (boolean + keyword + semantic) and notifies on
//! high-relevance hits.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rustss_db::{embeddings, notifications, saved_search_matches, saved_searches};
use rustss_embed::{Embedder, HashEmbedder};
use rustss_search::dsl;
use rustss_search::saved;

use crate::{Agent, AgentContext};

/// Candidate window size and recency (per saved search).
const CANDIDATE_LIMIT: i64 = 100;
const CANDIDATE_DAYS: i64 = 30;

/// Enqueues one match job per active saved search.
pub struct SavedSearchScanAgent;

#[async_trait]
impl Agent for SavedSearchScanAgent {
    fn name(&self) -> &'static str {
        "saved_search_scan"
    }

    async fn run(&self, ctx: &AgentContext, _payload: Value) -> anyhow::Result<Value> {
        let pool = ctx.db.pool();
        let searches = saved_searches::list_active(pool).await?;
        let mut enqueued = 0u64;
        for ss in &searches {
            let dedup = format!("saved_search_match:{}", ss.id);
            if rustss_db::jobs::enqueue(
                pool,
                "saved_search_match",
                json!({ "saved_search_id": ss.id }),
                Some(&dedup),
                None,
                3,
            )
            .await?
            .is_some()
            {
                enqueued += 1;
            }
        }
        Ok(json!({ "active": searches.len(), "enqueued": enqueued }))
    }
}

#[derive(Deserialize)]
struct Payload {
    saved_search_id: Uuid,
}

/// Matches new articles against a single saved search.
pub struct SavedSearchMatchAgent {
    embedder: Arc<dyn Embedder>,
}

impl SavedSearchMatchAgent {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self { embedder }
    }
}

impl Default for SavedSearchMatchAgent {
    fn default() -> Self {
        Self::new(Arc::new(HashEmbedder::default()))
    }
}

#[async_trait]
impl Agent for SavedSearchMatchAgent {
    fn name(&self) -> &'static str {
        "saved_search_match"
    }

    async fn run(&self, ctx: &AgentContext, payload: Value) -> anyhow::Result<Value> {
        let Payload { saved_search_id } = serde_json::from_value(payload)?;
        let pool = ctx.db.pool();

        let Some(ss) = saved_searches::get(pool, saved_search_id).await? else {
            return Ok(json!({ "skipped": "saved search gone" }));
        };

        let query = dsl::parse(&ss.query);
        let terms = query.positive_terms();
        let query_text = if terms.is_empty() {
            ss.query.clone()
        } else {
            terms.join(" ")
        };
        let query_vec = self.embedder.embed_one(&query_text).await?;

        let candidates = embeddings::candidates_for_query(
            pool,
            ss.user_id,
            &query_vec,
            self.embedder.model(),
            CANDIDATE_DAYS,
            CANDIDATE_LIMIT,
        )
        .await?;

        let mut new_matches = 0i64;
        let mut notified = 0u64;
        for cand in &candidates {
            let text_lower = format!("{} {}", cand.title, cand.content).to_ascii_lowercase();
            let tokens = dsl::tokenize(&text_lower);
            if !query.matches(&tokens, &text_lower) {
                continue;
            }
            let kw = saved::keyword_score(&query, &cand.title, &cand.content);
            let score = saved::combine(cand.similarity, kw.score);
            if score < ss.threshold {
                continue;
            }

            let reason = format!(
                "{}% match (semantic + keyword)",
                (score * 100.0).round() as i64
            );
            let created = saved_search_matches::upsert(
                pool,
                ss.id,
                cand.id,
                score,
                &json!(kw.matched),
                &reason,
            )
            .await?;
            if created {
                new_matches += 1;
            }

            if ss.notify_on_match
                && score >= ss.notify_threshold
                && saved_search_matches::claim_notify(pool, ss.id, cand.id).await?
            {
                notifications::create(
                    pool,
                    ss.user_id,
                    "saved_search_match",
                    &format!("New match for “{}”", ss.name),
                    &format!("{} ({}%)", cand.title, (score * 100.0).round() as i64),
                    Some(json!({ "saved_search_id": ss.id, "article_id": cand.id })),
                )
                .await?;
                notified += 1;
            }
        }

        saved_searches::record_matches(pool, ss.id, new_matches).await?;
        Ok(json!({
            "candidates": candidates.len(),
            "new_matches": new_matches,
            "notified": notified,
        }))
    }
}
