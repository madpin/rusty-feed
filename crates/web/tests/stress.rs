use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinSet;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rusty_feed_agents::{
    process, Agent, AgentContext, EmbedArticleAgent, ExtractContentAgent, FetchFeedAgent, Registry,
    SavedSearchMatchAgent, SummarizeArticleAgent,
};
use rusty_feed_cache::{Cache, CacheConfig};
use rusty_feed_core::feed_content::{
    ContentLifetime, SubscriptionContentMode, SubscriptionContentSettings,
};
use rusty_feed_db::models::NewArticle;
use rusty_feed_db::{
    api_tokens, articles, feeds, jobs, saved_searches, sessions, subscriptions, users, Db,
};
use rusty_feed_embed::HashEmbedder;
use rusty_feed_feeds::SafeFetchClient;
use rusty_feed_llm::OpenAiCompatClient;
use rusty_feed_web::auth::token;
use rusty_feed_web::{router, AppSettings, AppState, RateLimitSettings};

#[derive(Clone, Copy)]
struct StressConfig {
    users: usize,
    feeds: usize,
    articles_per_feed: usize,
    requests: usize,
    concurrency: usize,
    summaries: usize,
    enforce_targets: bool,
}

impl StressConfig {
    fn from_env() -> Self {
        Self {
            users: env_usize("STRESS_USERS", 8).clamp(2, 100),
            feeds: env_usize("STRESS_FEEDS", 8).clamp(2, 200),
            articles_per_feed: env_usize("STRESS_ARTICLES_PER_FEED", 25).clamp(5, 200),
            requests: env_usize("STRESS_REQUESTS", 500).clamp(50, 20_000),
            concurrency: env_usize("STRESS_CONCURRENCY", 8).clamp(1, 32),
            summaries: env_usize("STRESS_SUMMARIES", 8).clamp(1, 100),
            enforce_targets: std::env::var("STRESS_ENFORCE_TARGETS").as_deref() == Ok("1"),
        }
    }
}

#[derive(Clone)]
struct Credentials {
    cookie: String,
    token: String,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn per_minute(count: usize, elapsed: Duration) -> f64 {
    count as f64 * 60.0 / elapsed.as_secs_f64().max(0.001)
}

fn p95(samples: &[Duration]) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[((sorted.len() * 95).div_ceil(100)).saturating_sub(1)]
}

fn rss(base_url: &str, run: Uuid, feed: usize, articles: usize) -> String {
    let items = (0..articles)
        .map(|article| {
            format!(
                "<item><title>Rust systems {feed}-{article}</title><link>{base_url}/articles/{feed}/{article}</link><guid>{run}-{feed}-{article}</guid><description>Rust systems stress article {article} from shared feed {feed}.</description></item>"
            )
        })
        .collect::<String>();
    format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Stress {feed}</title><link>{base_url}</link>{items}</channel></rss>"
    )
}

async fn process_kind(
    db: &Db,
    context: &AgentContext,
    registry: Arc<Registry>,
    kind: &str,
    concurrency: usize,
) -> (usize, Duration) {
    let started = Instant::now();
    let mut workers = JoinSet::new();
    for worker in 0..concurrency {
        let db = db.clone();
        let context = context.clone();
        let registry = registry.clone();
        let kind = kind.to_string();
        workers.spawn(async move {
            let mut claimed = Vec::new();
            loop {
                let Some(job) = jobs::claim_next(
                    db.pool(),
                    &format!("stress-{worker}"),
                    std::slice::from_ref(&kind),
                )
                .await
                .unwrap() else {
                    break;
                };
                claimed.push(job.id);
                process(&context, &registry, job).await;
            }
            claimed
        });
    }

    let mut claimed = Vec::new();
    while let Some(result) = workers.join_next().await {
        claimed.extend(result.unwrap());
    }
    let unique: HashSet<_> = claimed.iter().copied().collect();
    assert_eq!(
        claimed.len(),
        unique.len(),
        "a queue item was claimed twice"
    );
    (claimed.len(), started.elapsed())
}

async fn make_cache(run: Uuid) -> Cache {
    let redis_url = std::env::var("TEST_REDIS_URL").ok();
    Cache::new(CacheConfig {
        enabled: redis_url.is_some(),
        redis_url,
        redis_password: None,
        l1_max_capacity: 50_000,
        namespace: format!("stress-{}", &run.to_string()[..8]),
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires dedicated TEST_DATABASE_URL; run with npm run test:stress"]
async fn full_stack_mixed_workload() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a dedicated disposable database");
    let config = StressConfig::from_env();
    let run = Uuid::now_v7();
    let db = Db::connect(&database_url).await.unwrap();
    db.migrate().await.unwrap();

    println!(
        "stress config: users={} feeds={} articles/feed={} requests={} concurrency={} summaries={} enforce={}",
        config.users,
        config.feeds,
        config.articles_per_feed,
        config.requests,
        config.concurrency,
        config.summaries,
        config.enforce_targets
    );

    let feed_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/favicon.ico"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&feed_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/articles/0/0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><body><article><h1>Extracted stress article</h1><p>{}</p></article></body></html>",
            "A deterministic extraction sentence. ".repeat(80)
        )))
        .expect(1)
        .mount(&feed_server)
        .await;
    for feed in 0..config.feeds {
        Mock::given(method("GET"))
            .and(path(format!("/feeds/{feed}.xml")))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/rss+xml")
                    .set_body_string(rss(&feed_server.uri(), run, feed, config.articles_per_feed)),
            )
            .expect(1)
            .mount(&feed_server)
            .await;
    }

    let mut user_ids = Vec::with_capacity(config.users);
    for index in 0..config.users {
        user_ids.push(
            users::create(
                db.pool(),
                &format!("stress-{run}-{index}@example.test"),
                None,
                None,
                "user",
                None,
            )
            .await
            .unwrap()
            .id,
        );
    }

    let mut subscriptions_set = JoinSet::new();
    for feed in 0..config.feeds {
        let url = format!("{}/feeds/{feed}.xml", feed_server.uri());
        for user_id in &user_ids {
            let db = db.clone();
            let url = url.clone();
            let user_id = *user_id;
            subscriptions_set.spawn(async move {
                let (shared, _) =
                    feeds::upsert_by_url(db.pool(), &url, &format!("Stress feed {feed}"), None)
                        .await
                        .unwrap();
                subscriptions::subscribe(db.pool(), user_id, shared.id, None)
                    .await
                    .unwrap();
                (feed, shared.id)
            });
        }
    }
    let mut ids_by_feed: HashMap<usize, HashSet<Uuid>> = HashMap::new();
    while let Some(result) = subscriptions_set.join_next().await {
        let (feed, id) = result.unwrap();
        ids_by_feed.entry(feed).or_default().insert(id);
    }
    assert!(ids_by_feed.values().all(|ids| ids.len() == 1));
    let feed_ids: Vec<_> = (0..config.feeds)
        .map(|feed| *ids_by_feed[&feed].iter().next().unwrap())
        .collect();
    let subscription_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_feeds WHERE user_id = ANY($1) AND feed_id = ANY($2)",
    )
    .bind(&user_ids)
    .bind(&feed_ids)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        subscription_count as usize,
        config.users * config.feeds,
        "personal subscriptions must not collapse"
    );

    let llm_server = MockServer::start().await;
    let summary_count = config
        .summaries
        .min(config.feeds * config.articles_per_feed);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "{\"summary\":\"Stress summary\",\"key_points\":[\"shared\"],\"topics\":[\"rust\"]}"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 25}
        })))
        .expect(summary_count as u64)
        .mount(&llm_server)
        .await;

    let feed_kind = format!("stress_fetch_{run}");
    let embed_kind = format!("stress_embed_{run}");
    let summary_kind = format!("stress_summary_{run}");
    let mut registry = Registry::with_defaults();
    registry.register(feed_kind.clone(), Arc::new(FetchFeedAgent));
    registry.register(embed_kind.clone(), Arc::new(EmbedArticleAgent::default()));
    registry.register(
        summary_kind.clone(),
        Arc::new(SummarizeArticleAgent::new(Some(Arc::new(
            OpenAiCompatClient::new(
                "stress",
                llm_server.uri(),
                Some("stress-key".to_string()),
                "gpt-4o-mini".to_string(),
            ),
        )))),
    );
    let registry = Arc::new(registry);
    let context = AgentContext::new(
        db.clone(),
        SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap(),
    )
    .with_eager_extraction(false);

    let mut enqueues = JoinSet::new();
    for feed_id in &feed_ids {
        for _ in 0..config.users {
            let db = db.clone();
            let kind = feed_kind.clone();
            let feed_id = *feed_id;
            let dedup = format!("{kind}:{feed_id}");
            enqueues.spawn(async move {
                jobs::enqueue(
                    db.pool(),
                    &kind,
                    json!({"feed_id": feed_id}),
                    Some(&dedup),
                    None,
                    1,
                )
                .await
                .unwrap()
                .is_some()
            });
        }
    }
    let mut inserted = 0;
    while let Some(result) = enqueues.join_next().await {
        inserted += usize::from(result.unwrap());
    }
    assert_eq!(
        inserted, config.feeds,
        "N subscribers must create one active fetch per shared feed"
    );

    let (fetched, feed_elapsed) = process_kind(
        &db,
        &context,
        registry.clone(),
        &feed_kind,
        config.concurrency,
    )
    .await;
    assert_eq!(fetched, config.feeds);
    let expected_articles = config.feeds * config.articles_per_feed;
    let article_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM articles WHERE feed_id = ANY($1)")
            .bind(&feed_ids)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(
        article_count as usize, expected_articles,
        "shared articles must be stored once, not once per subscriber"
    );

    subscriptions::update_content_settings(
        db.pool(),
        user_ids[0],
        feed_ids[0],
        SubscriptionContentSettings {
            mode: SubscriptionContentMode::ArticlePage,
            lifetime: ContentLifetime::Days(30),
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE feeds SET respect_robots_txt = false, extraction_delay_ms = 0 WHERE id = $1",
    )
    .bind(feed_ids[0])
    .execute(db.pool())
    .await
    .unwrap();
    let first_url = format!("{}/articles/0/0", feed_server.uri());
    let first_article = articles::find_by_url(db.pool(), &first_url)
        .await
        .unwrap()
        .unwrap();
    let extraction = ExtractContentAgent::default()
        .run(
            &context,
            json!({
                "article_id": first_article.id,
                "content_revision": first_article.content_revision
            }),
        )
        .await
        .unwrap();
    assert_eq!(extraction["status"], "success");

    let article_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM articles WHERE feed_id = ANY($1) ORDER BY id")
            .bind(&feed_ids)
            .fetch_all(db.pool())
            .await
            .unwrap();
    for article_id in &article_ids {
        let article = articles::find_by_id(db.pool(), *article_id)
            .await
            .unwrap()
            .unwrap();
        jobs::enqueue(
            db.pool(),
            &embed_kind,
            json!({
                "article_id": article.id,
                "content_revision": article.content_revision
            }),
            Some(&format!("{embed_kind}:{}", article.id)),
            None,
            1,
        )
        .await
        .unwrap()
        .unwrap();
    }
    let (embedded, embed_elapsed) = process_kind(
        &db,
        &context,
        registry.clone(),
        &embed_kind,
        config.concurrency,
    )
    .await;
    assert_eq!(embedded, expected_articles);

    let mut summary_inserted = 0;
    for article_id in article_ids.iter().take(summary_count) {
        let article = articles::find_by_id(db.pool(), *article_id)
            .await
            .unwrap()
            .unwrap();
        for _ in 0..config.users {
            summary_inserted += usize::from(
                jobs::enqueue(
                    db.pool(),
                    &summary_kind,
                    json!({
                        "article_id": article.id,
                        "content_revision": article.content_revision,
                        "user_id": user_ids[0]
                    }),
                    Some(&format!("{summary_kind}:{}", article.id)),
                    None,
                    1,
                )
                .await
                .unwrap()
                .is_some(),
            );
        }
    }
    assert_eq!(summary_inserted, summary_count);
    let (summarized, summary_elapsed) = process_kind(
        &db,
        &context,
        registry.clone(),
        &summary_kind,
        config.concurrency,
    )
    .await;
    assert_eq!(summarized, summary_count);

    let mut search_ids = Vec::new();
    for (index, user_id) in user_ids.iter().enumerate() {
        search_ids.push(
            saved_searches::create(
                db.pool(),
                *user_id,
                &format!("Stress search {index}"),
                "rust +systems",
                0.0,
                false,
                1.0,
            )
            .await
            .unwrap()
            .id,
        );
    }
    let match_agent = Arc::new(SavedSearchMatchAgent::default());
    let match_started = Instant::now();
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut matching = JoinSet::new();
    for search_id in search_ids {
        let agent = match_agent.clone();
        let context = context.clone();
        let semaphore = semaphore.clone();
        matching.spawn(async move {
            let _permit = semaphore.acquire_owned().await.unwrap();
            agent
                .run(&context, json!({"saved_search_id": search_id}))
                .await
                .unwrap()["candidates"]
                .as_u64()
                .unwrap() as usize
        });
    }
    let mut matched_candidates = 0;
    while let Some(result) = matching.join_next().await {
        matched_candidates += result.unwrap();
    }
    let match_elapsed = match_started.elapsed();
    assert!(matched_candidates >= config.users * config.articles_per_feed.min(100));

    let cache = make_cache(run).await;
    let cache_prefix = format!("stress:{run}:");
    let cache_started = Instant::now();
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut cache_tasks = JoinSet::new();
    for index in 0..config.requests {
        let cache = cache.clone();
        let key = format!("{cache_prefix}{index}");
        let semaphore = semaphore.clone();
        cache_tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.unwrap();
            cache.set(&key, &index, Duration::from_secs(120)).await;
            assert_eq!(cache.get::<usize>(&key).await, Some(index));
        });
    }
    while let Some(result) = cache_tasks.join_next().await {
        result.unwrap();
    }
    let cache_elapsed = cache_started.elapsed();

    let outsider = users::create(
        db.pool(),
        &format!("stress-outsider-{run}@example.test"),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let (outsider_feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://outsider.example/{run}.xml"),
        "Outsider feed",
        None,
    )
    .await
    .unwrap();
    subscriptions::subscribe(db.pool(), outsider.id, outsider_feed.id, None)
        .await
        .unwrap();
    let isolation_marker = format!("PRIVATE-STRESS-{run}");
    articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: outsider_feed.id,
            url: format!("https://outsider.example/articles/{run}"),
            comments_url: None,
            guid: Some(format!("outsider-{run}")),
            title: isolation_marker.clone(),
            author: None,
            content: "rust systems private tenant content".to_string(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap();

    let mut credentials = Vec::new();
    for (index, user_id) in user_ids.iter().enumerate() {
        let raw_session = format!("stress-session-{run}-{index}");
        sessions::create(
            db.pool(),
            *user_id,
            &token::hash(&raw_session),
            time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        )
        .await
        .unwrap();
        let raw_token = format!("stress-token-{run}-{index}");
        api_tokens::create(
            db.pool(),
            *user_id,
            &format!("Stress token {index}"),
            &token::hash(&raw_token),
            &["api".to_string()],
        )
        .await
        .unwrap();
        credentials.push(Credentials {
            cookie: format!("rusty_feed_sid={raw_session}"),
            token: raw_token,
        });
    }

    let settings = AppSettings {
        rate_limits: RateLimitSettings {
            auth_per_min: 0,
            api_per_min: 0,
            feed_add_per_min: 0,
            search_per_min: 0,
            summarize_per_min: 0,
            opml_per_min: 0,
            media_per_min: 0,
        },
        ..AppSettings::default()
    };
    let state = AppState::new(db.clone(), cache.clone(), settings, String::new)
        .with_fetch(SafeFetchClient::new_allow_private(Duration::from_secs(10)).unwrap())
        .with_embedder(Arc::new(HashEmbedder::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });
    let base_url = format!("http://{address}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    client
        .get(format!("{base_url}/api/health/ready"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .get(format!("{base_url}/api/v1/articles?limit=50"))
        .bearer_auth(&credentials[0].token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .get(format!("{base_url}/articles"))
        .header("cookie", &credentials[0].cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .get(format!("{base_url}/search?q=rust&mode=hybrid"))
        .header("cookie", &credentials[0].cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .post(format!(
            "{base_url}/api/v1/articles/{}/state",
            article_ids[0]
        ))
        .bearer_auth(&credentials[0].token)
        .json(&json!({"read": true, "bookmarked": false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut requests = JoinSet::new();
    for index in 0..config.requests {
        let client = client.clone();
        let base_url = base_url.clone();
        let credentials = credentials[index % credentials.len()].clone();
        let marker = isolation_marker.clone();
        let semaphore = semaphore.clone();
        let article_id = article_ids[index % article_ids.len()];
        requests.spawn(async move {
            let _permit = semaphore.acquire_owned().await.unwrap();
            let started = Instant::now();
            let (route, response) = match index % 6 {
                0 => (
                    "ready",
                    client
                        .get(format!("{base_url}/api/health/ready"))
                        .send()
                        .await
                        .unwrap(),
                ),
                1 => (
                    "api-list",
                    client
                        .get(format!("{base_url}/api/v1/articles?limit=50"))
                        .bearer_auth(&credentials.token)
                        .send()
                        .await
                        .unwrap(),
                ),
                2 => (
                    "library",
                    client
                        .get(format!("{base_url}/articles"))
                        .header("cookie", &credentials.cookie)
                        .send()
                        .await
                        .unwrap(),
                ),
                3 => (
                    "search",
                    client
                        .get(format!("{base_url}/search?q=rust&mode=hybrid"))
                        .header("cookie", &credentials.cookie)
                        .send()
                        .await
                        .unwrap(),
                ),
                4 => (
                    "api-write",
                    client
                        .post(format!("{base_url}/api/v1/articles/{article_id}/state"))
                        .bearer_auth(&credentials.token)
                        .json(&json!({"read": true, "bookmarked": false}))
                        .send()
                        .await
                        .unwrap(),
                ),
                _ => (
                    "metrics",
                    client
                        .get(format!("{base_url}/metrics"))
                        .send()
                        .await
                        .unwrap(),
                ),
            };
            let status = response.status();
            let body = response.text().await.unwrap();
            assert!(status.is_success(), "{route} returned {status}: {body}");
            assert!(
                !body.contains(&marker),
                "{route} leaked another tenant's article"
            );
            (route, started.elapsed())
        });
    }
    let mut latency: HashMap<&str, Vec<Duration>> = HashMap::new();
    while let Some(result) = requests.join_next().await {
        let (route, elapsed) = result.unwrap();
        latency.entry(route).or_default().push(elapsed);
    }

    let feed_rate = per_minute(fetched, feed_elapsed);
    let match_rate = per_minute(matched_candidates, match_elapsed);
    println!("\nfull-stack stress report");
    println!(
        "  feed ingest:        {:>8.0} feeds/min ({fetched} feeds, {expected_articles} shared articles)",
        feed_rate
    );
    println!(
        "  embedding:          {:>8.0} articles/min ({embedded} articles)",
        per_minute(embedded, embed_elapsed)
    );
    println!(
        "  summarization:      {:>8.0} articles/min ({summarized} unique summaries)",
        per_minute(summarized, summary_elapsed)
    );
    println!(
        "  saved-search match: {:>8.0} articles/min ({matched_candidates} candidates)",
        match_rate
    );
    println!(
        "  cache round trips:  {:>8.0} ops/min (L2 connected: {})",
        per_minute(config.requests, cache_elapsed),
        cache.l2_connected()
    );
    for route in [
        "ready",
        "api-list",
        "api-write",
        "library",
        "search",
        "metrics",
    ] {
        println!(
            "  HTTP {route:<10} p95 {:>8.2} ms",
            p95(&latency[route]).as_secs_f64() * 1_000.0
        );
    }

    if config.enforce_targets {
        assert!(
            feed_rate >= 200.0,
            "feed throughput {:.0}/min missed the 200/min target",
            feed_rate
        );
        assert!(
            match_rate >= 1_000.0,
            "saved-search throughput {:.0}/min missed the 1000/min target",
            match_rate
        );
        assert!(p95(&latency["api-list"]) < Duration::from_millis(75));
        assert!(p95(&latency["api-write"]) < Duration::from_millis(75));
        assert!(p95(&latency["library"]) < Duration::from_millis(50));
        assert!(p95(&latency["search"]) < Duration::from_millis(150));
    }

    let _ = shutdown_tx.send(());
    server.await.unwrap();
    cache.delete_prefix(&cache_prefix).await;
    sqlx::query("DELETE FROM job_queue WHERE kind = ANY($1)")
        .bind(vec![
            feed_kind.clone(),
            embed_kind.clone(),
            summary_kind.clone(),
        ])
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM agent_runs WHERE agent_name = ANY($1)")
        .bind(vec![feed_kind, embed_kind, summary_kind])
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM feeds WHERE id = ANY($1)")
        .bind(&feed_ids)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM feeds WHERE id = $1")
        .bind(outsider_feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut cleanup_users = user_ids;
    cleanup_users.push(outsider.id);
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(cleanup_users)
        .execute(db.pool())
        .await
        .unwrap();
}
