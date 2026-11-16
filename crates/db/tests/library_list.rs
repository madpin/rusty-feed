//! Library list oracle coverage (FND-234): the page-first `list_for_user`
//! must return exactly what the original join-everything query returned.
//! Tests skip when `TEST_DATABASE_URL` is unavailable.

use rusty_feed_db::{
    articles, bookmarks, categories, feeds, read_articles, seen_articles, subscriptions, users, Db,
    NewArticle,
};
use uuid::Uuid;

/// The Library query as it was before FND-234, kept verbatim as the oracle.
/// `{read_filter}` is substituted exactly like the original code did.
const OLD_LIST_FOR_USER_SQL: &str = "WITH RECURSIVE category_scope(id) AS ( \
             SELECT id FROM user_categories WHERE id = $3::uuid AND user_id = $1 \
             UNION \
             SELECT child.id FROM user_categories child \
             JOIN category_scope scope ON child.parent_id = scope.id \
             WHERE child.user_id = $1 \
         ) \
         SELECT a.id, a.feed_id, a.title, a.url, a.author, a.excerpt, a.image_url, a.published_at, \
                coalesce(nullif(uf.custom_name, ''), f.name) AS feed_name, \
                (r.article_id IS NOT NULL) AS is_read, \
                (seen.article_id IS NOT NULL) AS is_seen, \
                (b.article_id IS NOT NULL) AS is_bookmarked, \
                (a.summary IS NOT NULL AND a.summarized_at IS NOT NULL) AS has_summary, \
                coalesce(sc.member_count, 0) AS story_count, \
                af.feedback_value, uf.settings AS subscription_settings \
         FROM articles a \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN read_articles r ON r.article_id = a.id AND r.user_id = $1 \
         LEFT JOIN seen_articles seen ON seen.article_id = a.id AND seen.user_id = $1 \
         LEFT JOIN bookmarks b ON b.article_id = a.id AND b.user_id = $1 \
         LEFT JOIN article_feedback af ON af.article_id = a.id AND af.user_id = $1 \
         LEFT JOIN story_cluster_members scm ON scm.article_id = a.id \
         LEFT JOIN story_clusters sc ON sc.id = scm.cluster_id \
         WHERE ($2::uuid IS NULL OR a.feed_id = $2) \
           AND ($3::uuid IS NULL OR EXISTS (SELECT 1 FROM user_feed_categories ufc \
                JOIN category_scope scope ON scope.id = ufc.user_category_id \
                WHERE ufc.user_feed_id = uf.id)) \
           AND (NOT $4 OR NOT EXISTS (SELECT 1 FROM user_feed_categories ufc \
                WHERE ufc.user_feed_id = uf.id)) \
           AND ($5::text IS NULL OR uf.tags @> ARRAY[$5]::text[] \
               OR EXISTS (SELECT 1 FROM unnest(uf.tags) stored_tag \
                  WHERE lower(stored_tag) = lower($5))) \
           AND (NOT $8 OR (a.summary IS NOT NULL AND a.summarized_at IS NOT NULL)) \
           AND (NOT $9 OR seen.article_id IS NULL) {read_filter} \
         ORDER BY a.published_at DESC NULLS LAST, a.created_at DESC \
         LIMIT $6 OFFSET $7";

/// The oracle with the deterministic `a.id` tiebreak the new query adds.
fn oracle_sql(unread_only: bool) -> String {
    OLD_LIST_FOR_USER_SQL
        .replace(
            "{read_filter}",
            if unread_only {
                "AND r.article_id IS NULL"
            } else {
                ""
            },
        )
        .replace(
            "ORDER BY a.published_at DESC NULLS LAST, a.created_at DESC",
            "ORDER BY a.published_at DESC NULLS LAST, a.created_at DESC, a.id",
        )
}

#[allow(clippy::too_many_arguments)]
async fn oracle(
    db: &Db,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    summary_only: bool,
    limit: i64,
    offset: i64,
) -> Vec<articles::ArticleListItem> {
    sqlx::query_as::<_, articles::ArticleListItem>(&oracle_sql(unread_only))
        .bind(user_id)
        .bind(filter.feed_id)
        .bind(filter.category_id)
        .bind(filter.unfiled)
        .bind(filter.tag)
        .bind(limit)
        .bind(offset)
        .bind(summary_only)
        .bind(filter.unseen_only)
        .fetch_all(db.pool())
        .await
        .expect("oracle query")
}

async fn test_db() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect to test database");
    db.migrate().await.expect("run migrations");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7())
}

async fn user(db: &Db, prefix: &str) -> Uuid {
    users::create(
        db.pool(),
        &format!("{}@example.test", unique(prefix)),
        None,
        None,
        "user",
        None,
    )
    .await
    .expect("create user")
    .id
}

async fn feed(db: &Db, name: &str) -> Uuid {
    feeds::upsert_by_url(
        db.pool(),
        &format!("https://library-list.example/{}/feed.xml", unique(name)),
        name,
        None,
    )
    .await
    .expect("create feed")
    .0
    .id
}

/// Insert an article and pin its ordering columns. `published_hours_ago` of
/// `None` stores a NULL `published_at`.
async fn article(
    db: &Db,
    feed_id: Uuid,
    title: &str,
    published_hours_ago: Option<i32>,
    created_minutes_ago: i32,
    summary: Option<(&str, bool)>,
) -> Uuid {
    let id = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id,
            url: format!("https://library-list.example/articles/{}", Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: title.to_string(),
            author: Some(format!("{title} author")),
            content: title.to_string(),
            excerpt: Some(format!("{title} excerpt")),
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .expect("create article")
    .0
    .id;
    sqlx::query(
        "UPDATE articles SET \
             published_at = CASE WHEN $2::int IS NULL THEN NULL \
                 ELSE timestamptz '2026-01-01 00:00:00+00' - make_interval(hours => $2::int) END, \
             created_at = timestamptz '2026-01-01 00:00:00+00' - make_interval(mins => $3::int), \
             summary = $4, \
             summarized_at = CASE WHEN $5 THEN now() ELSE NULL END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(published_hours_ago)
    .bind(created_minutes_ago)
    .bind(summary.map(|(text, _)| text))
    .bind(summary.is_some_and(|(_, ready)| ready))
    .execute(db.pool())
    .await
    .expect("pin article ordering");
    id
}

async fn feedback(db: &Db, user_id: Uuid, article_id: Uuid, value: f64) {
    sqlx::query(
        "INSERT INTO article_feedback (id, user_id, article_id, feedback_type, feedback_value) \
         VALUES ($1, $2, $3, 'explicit', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(article_id)
    .bind(value)
    .execute(db.pool())
    .await
    .expect("insert feedback");
}

async fn cluster(db: &Db, members: &[Uuid]) {
    let cluster_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO story_clusters (id, title, representative_article_id, member_count, model) \
         VALUES ($1, 'Shared story', $2, $3, 'test')",
    )
    .bind(cluster_id)
    .bind(members[0])
    .bind(members.len() as i32)
    .execute(db.pool())
    .await
    .expect("insert cluster");
    for member in members {
        sqlx::query(
            "INSERT INTO story_cluster_members (article_id, cluster_id, content_revision, similarity) \
             VALUES ($1, $2, 1, 0.9)",
        )
        .bind(member)
        .bind(cluster_id)
        .execute(db.pool())
        .await
        .expect("insert cluster member");
    }
}

async fn add_folder(db: &Db, user_id: Uuid, feed_id: Uuid, category_id: Uuid) {
    sqlx::query(
        "INSERT INTO user_feed_categories (id, user_feed_id, user_category_id) \
         SELECT $1, uf.id, $4 FROM user_feeds uf WHERE uf.user_id = $2 AND uf.feed_id = $3",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(feed_id)
    .bind(category_id)
    .execute(db.pool())
    .await
    .expect("add extra folder");
}

/// One named Library scope to compare.
struct Scope {
    label: &'static str,
    feed_id: Option<Uuid>,
    category_id: Option<Uuid>,
    unfiled: bool,
    tag: Option<&'static str>,
}

fn scope(label: &'static str) -> Scope {
    Scope {
        label,
        feed_id: None,
        category_id: None,
        unfiled: false,
        tag: None,
    }
}

fn rows(items: &[articles::ArticleListItem]) -> Vec<String> {
    items.iter().map(|item| format!("{item:?}")).collect()
}

/// Compare the new query with the oracle for every flag combination, two page
/// sizes and three offsets. Returns how many comparisons were made.
async fn assert_matches_oracle(db: &Db, user_id: Uuid, scopes: &[Scope]) -> usize {
    let mut compared = 0;
    for scope in scopes {
        for unread_only in [false, true] {
            for summary_only in [false, true] {
                for unseen_only in [false, true] {
                    for limit in [5_i64, 20] {
                        for offset in [0_i64, 3, 17] {
                            let filter = articles::LibraryFilter {
                                feed_id: scope.feed_id,
                                category_id: scope.category_id,
                                unfiled: scope.unfiled,
                                tag: scope.tag,
                                unseen_only,
                            };
                            let expected = oracle(
                                db,
                                user_id,
                                filter,
                                unread_only,
                                summary_only,
                                limit,
                                offset,
                            )
                            .await;
                            let actual = articles::list_for_user(
                                db.pool(),
                                user_id,
                                filter,
                                unread_only,
                                summary_only,
                                limit,
                                offset,
                            )
                            .await
                            .expect("list for user");
                            assert_eq!(
                                rows(&actual),
                                rows(&expected),
                                "scope={} unread={unread_only} summary={summary_only} \
                                 unseen={unseen_only} limit={limit} offset={offset}",
                                scope.label
                            );
                            compared += 1;
                        }
                    }
                }
            }
        }
    }
    compared
}

#[tokio::test]
async fn library_page_matches_the_original_query_for_every_filter() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let alice = user(&db, "library-alice").await;
    let bob = user(&db, "library-bob").await;

    let feed_ids = [
        feed(&db, "Async feed").await,
        feed(&db, "Web feed").await,
        feed(&db, "News feed").await,
        feed(&db, "Loose feed").await,
        feed(&db, "Bob only feed").await,
    ];
    for feed_id in &feed_ids[..4] {
        subscriptions::subscribe(db.pool(), alice, *feed_id, None)
            .await
            .expect("alice subscribes");
    }
    for feed_id in [feed_ids[1], feed_ids[2], feed_ids[4]] {
        subscriptions::subscribe(db.pool(), bob, feed_id, None)
            .await
            .expect("bob subscribes");
    }

    // Alice: Tech > Rust > Async, plus a separate News root.
    let tech = categories::create(db.pool(), alice, &unique("Tech"), None)
        .await
        .expect("tech");
    let rust = categories::create(db.pool(), alice, &unique("Rust"), Some(tech.id))
        .await
        .expect("rust");
    let asynchronous = categories::create(db.pool(), alice, &unique("Async"), Some(rust.id))
        .await
        .expect("async");
    let news = categories::create(db.pool(), alice, &unique("News"), None)
        .await
        .expect("news");
    let bob_root = categories::create(db.pool(), bob, &unique("Bob"), None)
        .await
        .expect("bob root");
    for (user_id, feed_id, tags, folder) in [
        (
            alice,
            feed_ids[0],
            vec!["Rust".to_string(), "Work".to_string()],
            Some(asynchronous.id),
        ),
        (alice, feed_ids[1], vec!["rust".to_string()], Some(tech.id)),
        (alice, feed_ids[2], vec!["WORK".to_string()], Some(news.id)),
        (alice, feed_ids[3], Vec::new(), None),
        (
            bob,
            feed_ids[1],
            vec!["rust".to_string()],
            Some(bob_root.id),
        ),
    ] {
        subscriptions::update_organization(db.pool(), user_id, feed_id, &tags, folder)
            .await
            .expect("organize subscription");
    }
    // A subscription filed under two folders must still appear once.
    add_folder(&db, alice, feed_ids[1], news.id).await;
    subscriptions::set_custom_name(db.pool(), alice, feed_ids[1], Some("Alice Web"))
        .await
        .expect("custom name");
    subscriptions::set_custom_name(db.pool(), alice, feed_ids[2], Some(""))
        .await
        .expect("empty custom name");
    sqlx::query(
        "UPDATE user_feeds SET settings = '{\"entry_filters\": {\"block\": \"nothing\"}}' \
         WHERE user_id = $1 AND feed_id = $2",
    )
    .bind(alice)
    .bind(feed_ids[0])
    .execute(db.pool())
    .await
    .expect("subscription settings");

    // Twelve articles per feed with NULL published_at, equal published_at
    // with different created_at, and fully equal timestamps across feeds so
    // the id tiebreak decides.
    let mut by_feed = vec![Vec::new(); feed_ids.len()];
    for (feed_index, feed_id) in feed_ids.iter().enumerate() {
        for slot in 0..12_i32 {
            let published = (slot % 5 != 4).then_some(slot % 4);
            let summary = match slot {
                7 => Some(("Pending summary", false)),
                slot if slot % 3 == 0 => Some(("Ready summary", true)),
                _ => None,
            };
            let id = article(
                &db,
                *feed_id,
                &format!("Feed {feed_index} story {slot}"),
                published,
                slot % 3,
                summary,
            )
            .await;
            by_feed[feed_index].push(id);
        }
    }

    for (feed_index, ids) in by_feed.iter().enumerate() {
        for (slot, id) in ids.iter().enumerate() {
            if (feed_index + slot) % 3 == 0 {
                read_articles::mark_read(db.pool(), alice, *id)
                    .await
                    .expect("alice read");
            }
            if (feed_index + slot) % 4 == 1 {
                seen_articles::mark_seen(db.pool(), alice, *id)
                    .await
                    .expect("alice seen");
            }
            if slot % 6 == 2 {
                bookmarks::add(db.pool(), alice, *id)
                    .await
                    .expect("alice bookmark");
            }
            if slot % 5 == 1 {
                feedback(&db, alice, *id, if slot % 2 == 0 { 1.0 } else { -1.0 }).await;
            }
            if (feed_index + slot) % 2 == 0 {
                read_articles::mark_read(db.pool(), bob, *id)
                    .await
                    .expect("bob read");
            }
            if slot % 4 == 3 {
                seen_articles::mark_seen(db.pool(), bob, *id)
                    .await
                    .expect("bob seen");
                bookmarks::add(db.pool(), bob, *id)
                    .await
                    .expect("bob bookmark");
                feedback(&db, bob, *id, 0.5).await;
            }
        }
    }
    cluster(&db, &[by_feed[0][1], by_feed[1][1], by_feed[2][1]]).await;
    cluster(&db, &[by_feed[1][5], by_feed[4][5]]).await;

    // Sanity: the dataset really has NULL dates last and cross-feed ties.
    let everything = articles::list_for_user(
        db.pool(),
        alice,
        articles::LibraryFilter::default(),
        false,
        false,
        100,
        0,
    )
    .await
    .expect("alice everything");
    assert_eq!(everything.len(), 48);
    assert!(everything[..40]
        .iter()
        .all(|item| item.published_at.is_some()));
    assert!(everything[40..]
        .iter()
        .all(|item| item.published_at.is_none()));
    assert!(everything.iter().any(|item| item.story_count == 3));
    assert!(everything.iter().all(|item| !by_feed[4].contains(&item.id)));

    let alice_scopes = vec![
        scope("everything"),
        Scope {
            feed_id: Some(feed_ids[0]),
            ..scope("feed async")
        },
        Scope {
            feed_id: Some(feed_ids[3]),
            ..scope("feed loose")
        },
        Scope {
            category_id: Some(tech.id),
            ..scope("folder tech with children")
        },
        Scope {
            category_id: Some(rust.id),
            ..scope("folder rust with child")
        },
        Scope {
            category_id: Some(asynchronous.id),
            ..scope("folder async leaf")
        },
        Scope {
            category_id: Some(news.id),
            ..scope("folder news")
        },
        Scope {
            unfiled: true,
            ..scope("unfiled")
        },
        Scope {
            tag: Some("rust"),
            ..scope("tag rust")
        },
        Scope {
            tag: Some("RUST"),
            ..scope("tag RUST")
        },
        Scope {
            tag: Some("work"),
            ..scope("tag work")
        },
        Scope {
            tag: Some("missing"),
            ..scope("tag missing")
        },
        Scope {
            category_id: Some(tech.id),
            tag: Some("work"),
            ..scope("folder tech + tag work")
        },
        Scope {
            feed_id: Some(feed_ids[1]),
            category_id: Some(news.id),
            ..scope("feed web + folder news")
        },
        Scope {
            feed_id: Some(feed_ids[2]),
            category_id: Some(tech.id),
            ..scope("feed news + folder tech")
        },
        Scope {
            category_id: Some(bob_root.id),
            ..scope("foreign folder")
        },
        Scope {
            feed_id: Some(feed_ids[4]),
            ..scope("unsubscribed feed")
        },
    ];
    let compared = assert_matches_oracle(&db, alice, &alice_scopes).await;
    assert_eq!(compared, alice_scopes.len() * 48);

    // Bob shares two feeds with Alice; his flags must be his own.
    let bob_scopes = vec![
        scope("everything"),
        Scope {
            feed_id: Some(feed_ids[1]),
            ..scope("feed web")
        },
        Scope {
            category_id: Some(bob_root.id),
            ..scope("folder bob")
        },
        Scope {
            unfiled: true,
            ..scope("unfiled")
        },
        Scope {
            tag: Some("Rust"),
            ..scope("tag Rust")
        },
        Scope {
            category_id: Some(tech.id),
            ..scope("foreign folder")
        },
    ];
    assert_matches_oracle(&db, bob, &bob_scopes).await;

    let bob_everything = articles::list_for_user(
        db.pool(),
        bob,
        articles::LibraryFilter::default(),
        false,
        false,
        100,
        0,
    )
    .await
    .expect("bob everything");
    assert_eq!(bob_everything.len(), 36);
    assert!(bob_everything
        .iter()
        .all(|item| !by_feed[0].contains(&item.id) && !by_feed[3].contains(&item.id)));
    let shared = bob_everything
        .iter()
        .find(|item| item.feed_id == feed_ids[1])
        .expect("shared feed row");
    assert_eq!(shared.feed_name, "Web feed");
    assert!(shared.subscription_settings.is_none());
}

/// What the web Library does: fetch `fetch` rows, advance by `step`, seek the
/// next page from fetched row `step - 1`. Asserts every cursor page fetched
/// exactly the offset page's rows, and returns the page count, whether a
/// boundary fell inside an equal-timestamp run, and whether one reached the
/// undated rows.
async fn assert_cursor_walk_matches_offset_walk(
    db: &Db,
    user_id: Uuid,
    filter: articles::LibraryFilter<'_>,
    unread_only: bool,
    step: i64,
    fetch: i64,
) -> (usize, bool, bool) {
    let mut offset = 0;
    let mut cursor = None;
    let mut pages = 0;
    let mut boundary_in_tie = false;
    let mut boundary_at_nulls = false;
    loop {
        let by_offset = articles::list_for_user_page(
            db.pool(),
            user_id,
            filter,
            unread_only,
            false,
            fetch,
            articles::LibraryPosition::Offset(offset),
        )
        .await
        .expect("offset page");
        if let Some(cursor) = cursor {
            let by_cursor = articles::list_for_user_page(
                db.pool(),
                user_id,
                filter,
                unread_only,
                false,
                fetch,
                articles::LibraryPosition::After(cursor),
            )
            .await
            .expect("cursor page");
            assert_eq!(
                by_cursor
                    .iter()
                    .map(|row| format!("{:?}", row.item))
                    .collect::<Vec<_>>(),
                by_offset
                    .iter()
                    .map(|row| format!("{:?}", row.item))
                    .collect::<Vec<_>>(),
                "unread={unread_only} step={step} fetch={fetch} offset={offset} \
                 filter={filter:?}"
            );
        }
        pages += 1;
        if by_offset.len() as i64 <= step {
            break;
        }
        let last = by_offset[step as usize - 1].cursor;
        let next = by_offset[step as usize].cursor;
        boundary_in_tie |= last.published_at.is_some()
            && last.published_at == next.published_at
            && last.created_at == next.created_at;
        boundary_at_nulls |= next.published_at.is_none();
        cursor = Some(last);
        offset += step;
    }
    (pages, boundary_in_tie, boundary_at_nulls)
}

#[tokio::test]
async fn cursor_pages_match_offset_pages_across_ties_and_undated_rows() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let reader = user(&db, "library-cursor").await;
    let outsider = user(&db, "library-cursor-outsider").await;
    let feed_ids = [
        feed(&db, "Cursor one").await,
        feed(&db, "Cursor two").await,
        feed(&db, "Cursor three").await,
    ];
    for feed_id in feed_ids {
        subscriptions::subscribe(db.pool(), reader, feed_id, None)
            .await
            .expect("subscribe");
    }
    subscriptions::subscribe(db.pool(), outsider, feed_ids[0], None)
        .await
        .expect("outsider subscribes");
    let parent = categories::create(db.pool(), reader, &unique("Cursor parent"), None)
        .await
        .expect("parent folder");
    let child = categories::create(db.pool(), reader, &unique("Cursor child"), Some(parent.id))
        .await
        .expect("child folder");
    subscriptions::update_organization(db.pool(), reader, feed_ids[0], &[], Some(parent.id))
        .await
        .expect("file feed one");
    subscriptions::update_organization(db.pool(), reader, feed_ids[1], &[], Some(child.id))
        .await
        .expect("file feed two");

    // 3 feeds x 14 rows: runs of identical (published, created) pairs inside
    // and across feeds, and a block of undated rows sharing created_at values.
    for (feed_index, feed_id) in feed_ids.iter().enumerate() {
        for slot in 0..14_i32 {
            let published = (slot < 10).then_some(slot / 3);
            let id = article(
                &db,
                *feed_id,
                &format!("Cursor {feed_index}/{slot}"),
                published,
                slot % 2,
                None,
            )
            .await;
            if (feed_index as i32 + slot) % 3 == 0 {
                read_articles::mark_read(db.pool(), reader, id)
                    .await
                    .expect("read");
            }
            if slot % 4 == 0 {
                read_articles::mark_read(db.pool(), outsider, id)
                    .await
                    .expect("outsider read");
            }
        }
    }

    let mut saw_tie = false;
    let mut saw_nulls = false;
    for category_id in [None, Some(parent.id)] {
        let filter = articles::LibraryFilter {
            category_id,
            ..articles::LibraryFilter::default()
        };
        for unread_only in [false, true] {
            for step in 1..=7_i64 {
                for fetch in [step + 1, step * 5 + 1] {
                    let (pages, tie, nulls) = assert_cursor_walk_matches_offset_walk(
                        &db,
                        reader,
                        filter,
                        unread_only,
                        step,
                        fetch,
                    )
                    .await;
                    saw_tie |= tie;
                    saw_nulls |= nulls;
                    if step <= 4 && category_id.is_none() {
                        assert!(pages >= 4, "step {step} walked only {pages} pages");
                    }
                }
            }
        }
    }
    assert!(
        saw_tie,
        "no page boundary fell inside an equal-timestamp run"
    );
    assert!(saw_nulls, "no page boundary reached the undated rows");

    // A cursor from one user's walk never exposes another user's feeds.
    let first = articles::list_for_user_page(
        db.pool(),
        reader,
        articles::LibraryFilter::default(),
        false,
        false,
        3,
        articles::LibraryPosition::Offset(0),
    )
    .await
    .expect("reader first page");
    let outsider_after = articles::list_for_user_page(
        db.pool(),
        outsider,
        articles::LibraryFilter::default(),
        false,
        false,
        100,
        articles::LibraryPosition::After(first[2].cursor),
    )
    .await
    .expect("outsider seek");
    assert!(outsider_after
        .iter()
        .all(|row| row.item.feed_id == feed_ids[0]));
}
