use rustss_db::{
    acquisition_profiles, articles, feeds, integrations, models::NewArticle, passkeys,
    rewrite_rules, ui_customizations, users, youtube_metadata, Db,
};

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7())
}

async fn connect() -> Option<Db> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let db = Db::connect(&url).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

#[tokio::test]
async fn passkey_challenges_are_one_use_and_credentials_are_tenant_scoped() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let alice = users::create(
        db.pool(),
        &format!("{}@example.test", unique("passkey-a")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let bob = users::create(
        db.pool(),
        &format!("{}@example.test", unique("passkey-b")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let ceremony = passkeys::create_ceremony(
        db.pool(),
        alice.id,
        "registration",
        &serde_json::json!({"challenge": "server-only"}),
    )
    .await
    .unwrap();
    assert!(
        passkeys::take_ceremony(db.pool(), ceremony, bob.id, "registration")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        passkeys::take_ceremony(db.pool(), ceremony, alice.id, "registration")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        passkeys::take_ceremony(db.pool(), ceremony, alice.id, "registration")
            .await
            .unwrap()
            .is_none()
    );

    let credential_id = unique("public-credential").into_bytes();
    let credential = passkeys::create(
        db.pool(),
        alice.id,
        "Laptop",
        &credential_id,
        &serde_json::json!({"public": true}),
    )
    .await
    .unwrap();
    assert_eq!(passkeys::list(db.pool(), alice.id).await.unwrap().len(), 1);
    assert!(passkeys::list(db.pool(), bob.id).await.unwrap().is_empty());
    assert!(!passkeys::delete(db.pool(), credential.id, bob.id)
        .await
        .unwrap());
}

#[tokio::test]
async fn integration_profiles_encrypt_outside_repository_and_history_is_personal() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("integration")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let profile = integrations::create_profile(
        db.pool(),
        user.id,
        "Research archive",
        "webhook",
        "https://hooks.example.test/rustss",
        Some("ciphertext-not-plaintext"),
        "hmac",
        "{{title}} {{url}}",
        &["research".into()],
        &["article.saved".into()],
        5,
        30,
        true,
    )
    .await
    .unwrap();
    let delivery = integrations::queue_delivery(db.pool(), &profile, None, "test")
        .await
        .unwrap();
    assert_eq!(
        integrations::find_delivery(db.pool(), delivery.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
    integrations::mark_delivering(db.pool(), delivery.id)
        .await
        .unwrap();
    integrations::mark_delivered(db.pool(), delivery.id, 204)
        .await
        .unwrap();
    assert_eq!(
        integrations::list_deliveries(db.pool(), user.id, 10)
            .await
            .unwrap()[0]
            .response_status,
        Some(204)
    );
    assert!(integrations::rotate_credential(
        db.pool(),
        profile.id,
        user.id,
        Some("replacement-ciphertext"),
        "bearer",
    )
    .await
    .unwrap());
    let rotated = integrations::find_owned(db.pool(), profile.id, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rotated.credential_ciphertext.as_deref(),
        Some("replacement-ciphertext")
    );
}

#[tokio::test]
async fn customization_history_restores_and_youtube_metadata_is_shared() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let user = users::create(
        db.pool(),
        &format!("{}@example.test", unique("customization")),
        None,
        None,
        "user",
        None,
    )
    .await
    .unwrap();
    let first = ui_customizations::create_version(
        db.pool(),
        Some(user.id),
        "css",
        ".reader { max-width: 70ch; }",
        user.id,
    )
    .await
    .unwrap();
    ui_customizations::create_version(
        db.pool(),
        Some(user.id),
        "css",
        ".reader { max-width: 62ch; }",
        user.id,
    )
    .await
    .unwrap();
    ui_customizations::restore(db.pool(), first.id, Some(user.id), "css", user.id)
        .await
        .unwrap();
    assert_eq!(
        ui_customizations::current_css(db.pool(), user.id)
            .await
            .unwrap()
            .unwrap()
            .content,
        ".reader { max-width: 70ch; }"
    );

    youtube_metadata::reserve(db.pool(), "dQw4w9WgXcQ")
        .await
        .unwrap();
    youtube_metadata::mark_ready(db.pool(), "dQw4w9WgXcQ", 213, 30)
        .await
        .unwrap();
    let metadata = youtube_metadata::list(db.pool(), &["dQw4w9WgXcQ".into()])
        .await
        .unwrap();
    assert_eq!(metadata[0].duration_secs, Some(213));
}

#[tokio::test]
async fn acquisition_and_rewrite_profiles_are_shared_versioned_and_audited() {
    let Some(db) = connect().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let admin = users::create(
        db.pool(),
        &format!("{}@example.test", unique("acquisition")),
        None,
        None,
        "admin",
        None,
    )
    .await
    .unwrap();
    let profile = acquisition_profiles::create(
        db.pool(),
        admin.id,
        &unique("Members feed"),
        &["example.test".into()],
        Some("/members/"),
        Some("encrypted"),
        true,
        true,
        true,
        Some(time::OffsetDateTime::now_utc() + time::Duration::days(30)),
    )
    .await
    .unwrap();
    let (feed, _) = feeds::upsert_by_url(
        db.pool(),
        &format!("https://example.test/members/{}.xml", uuid::Uuid::now_v7()),
        "Members",
        None,
    )
    .await
    .unwrap();
    acquisition_profiles::assign(db.pool(), admin.id, feed.id, Some(profile.id))
        .await
        .unwrap();
    assert_eq!(
        acquisition_profiles::for_feed(db.pool(), feed.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        profile.id
    );
    let audit = acquisition_profiles::list_audit(db.pool(), 100)
        .await
        .unwrap();
    assert!(audit.iter().any(|entry| {
        entry.action == "created" && entry.detail["cookie"] == true && entry.detail["proxy"] == true
    }));
    let first = rewrite_rules::create(
        db.pool(),
        feed.id,
        admin.id,
        &serde_json::json!([{"name":"Remove promo","action":"remove","selector":".promo","enabled":true}]),
    )
    .await
    .unwrap();
    let second = rewrite_rules::create(db.pool(), feed.id, admin.id, &serde_json::json!([]))
        .await
        .unwrap();
    assert_eq!(second.version, first.version + 1);
    assert_eq!(
        rewrite_rules::list(db.pool(), feed.id, 10)
            .await
            .unwrap()
            .len(),
        2
    );

    sqlx::query("UPDATE feeds SET etag = 'old', last_modified = 'yesterday' WHERE id = $1")
        .bind(feed.id)
        .execute(db.pool())
        .await
        .unwrap();
    feeds::clear_fetch_validators(db.pool(), feed.id)
        .await
        .unwrap();
    let validators: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT etag, last_modified FROM feeds WHERE id = $1")
            .bind(feed.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(validators, (None, None));

    let article = articles::upsert(
        db.pool(),
        &NewArticle {
            feed_id: feed.id,
            url: format!("https://example.test/members/{}", uuid::Uuid::now_v7()),
            comments_url: None,
            guid: None,
            title: "Rewrite source".into(),
            author: None,
            content: "<p>Source</p>".into(),
            excerpt: None,
            image_url: None,
            enclosures: Vec::new(),
            published_at: None,
        },
    )
    .await
    .unwrap()
    .0;
    sqlx::query(
        "UPDATE articles SET article_page_content = '<p>Old rules</p>', \
         article_page_processing_version = 2 WHERE id = $1",
    )
    .bind(article.id)
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(
        articles::invalidate_article_page_processing_for_feed(db.pool(), feed.id)
            .await
            .unwrap(),
        1
    );
    let version: i32 =
        sqlx::query_scalar("SELECT article_page_processing_version FROM articles WHERE id = $1")
            .bind(article.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(version, 0);
}
