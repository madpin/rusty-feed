//! Rusty Feed admin/maintenance CLI.
//!
//! Commands intentionally use the same validated configuration as the server,
//! so maintenance cannot create an incompatible embedding space.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("migrate") => run(migrate().await),
        Some("config-check") => run(config_check()),
        Some("create-admin") => run(create_admin().await),
        Some("fetch-model") => run(fetch_model().await),
        Some("backfill") => run(queue_embedding_backfill(false).await),
        Some("canonicalize-urls") => {
            run(canonicalize_urls(args.iter().any(|arg| arg == "--apply")).await)
        }
        Some("reindex") if args.iter().any(|arg| arg == "--confirm") => {
            run(queue_embedding_backfill(true).await)
        }
        Some("reindex") => {
            eprintln!(
                "error: reindex replaces every stored vector; stop workers and rerun with --confirm"
            );
            ExitCode::from(2)
        }
        Some("demo-reset") if args.iter().any(|arg| arg == "--confirm") => run(demo_reset().await),
        Some("demo-reset") => {
            eprintln!(
                "error: demo-reset deletes ALL demo users and their data; rerun with --confirm"
            );
            ExitCode::from(2)
        }
        Some("--version") | Some("-V") => {
            println!("{}", rusty_feed_core::banner());
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | Some("help") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("error: unknown command `{other}`\n");
            print_help();
            ExitCode::from(2)
        }
    }
}

/// Report legacy URL collisions and optionally update only conflict-free keys.
async fn canonicalize_urls(apply: bool) -> anyhow::Result<()> {
    let config = rusty_feed_core::AppConfig::load()?;
    let policy = config.tracking_url_policy().map_err(anyhow::Error::msg)?;
    let db = rusty_feed_db::Db::connect(&config.database_url).await?;
    db.migrate().await?;
    let plan = rusty_feed_db::url_backfill::plan(db.pool(), &policy).await?;
    println!(
        "canonical URL plan: {} safe change(s), {} collision(s), {} unchanged",
        plan.changes.len(),
        plan.collisions.len(),
        plan.unchanged
    );
    for collision in plan.collisions.iter().take(50) {
        println!(
            "collision: article {} cannot change {} -> {}",
            collision.article_id, collision.current_url, collision.canonical_url
        );
    }
    if plan.collisions.len() > 50 {
        println!(
            "{} additional collision(s) omitted",
            plan.collisions.len() - 50
        );
    }
    if apply {
        let changed = rusty_feed_db::url_backfill::apply(db.pool(), &plan).await?;
        println!("updated {changed} collision-free article key(s)");
    } else if !plan.changes.is_empty() {
        println!("dry run only; rerun with --apply to update collision-free keys");
    }
    Ok(())
}

/// Download and load the configured local ONNX model, then run a dimensional
/// readiness probe. A populated cache can subsequently be copied to an
/// air-gapped host without runtime network access.
async fn fetch_model() -> anyhow::Result<()> {
    let config = rusty_feed_core::AppConfig::load()?;
    if !config.embeddings_enabled {
        println!(
            "embeddings are disabled (EMBEDDINGS_ENABLED=false); nothing to fetch. \
             Set EMBEDDINGS_ENABLED=true to use a model."
        );
        return Ok(());
    }
    let embedder = rusty_feed_embed::build_embedder(rusty_feed_embed::EmbedConfig {
        provider: config.embedding_provider,
        model: config.embedding_model,
        openai_api_key: config
            .openai_api_key
            .as_ref()
            .map(|secret| secret.expose().to_string()),
        openai_base_url: config.openai_base_url,
        model_cache_dir: config.model_cache_dir.clone(),
    })?;
    let vector = embedder
        .embed_one("Rusty Feed model readiness probe")
        .await?;
    if vector.len() != rusty_feed_embed::EMBEDDING_DIM {
        anyhow::bail!(
            "model returned {} dimensions; Rusty Feed requires {}",
            vector.len(),
            rusty_feed_embed::EMBEDDING_DIM
        );
    }
    println!(
        "model {} is ready ({} dimensions) in {}",
        embedder.model(),
        vector.len(),
        config.model_cache_dir.display()
    );
    Ok(())
}

/// Delete every ephemeral demo account (and its data). Scoped strictly to
/// `is_demo = true`, so real users are never touched (docs/PUBLIC_LAUNCH_PLAN.md §7.3).
async fn demo_reset() -> anyhow::Result<()> {
    let config = rusty_feed_core::AppConfig::load()?;
    let db = rusty_feed_db::Db::connect(&config.database_url).await?;
    db.migrate().await?;
    let deleted = rusty_feed_db::users::delete_all_demo(db.pool()).await?;
    println!("demo reset complete: {deleted} demo account(s) deleted");
    Ok(())
}

/// Create or promote an administrator. Credentials are read from environment
/// variables so the password never appears in the process argument list.
async fn create_admin() -> anyhow::Result<()> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};

    let config = rusty_feed_core::AppConfig::load()?;
    let email = std::env::var("RUSTY_FEED_ADMIN_EMAIL")
        .map_err(|_| anyhow::anyhow!("RUSTY_FEED_ADMIN_EMAIL must be set"))?;
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.len() > 320 {
        anyhow::bail!("RUSTY_FEED_ADMIN_EMAIL is not a valid email address");
    }
    let db = rusty_feed_db::Db::connect(&config.database_url).await?;
    db.migrate().await?;
    if let Some(user) = rusty_feed_db::users::find_by_email(db.pool(), &email).await? {
        rusty_feed_db::users::set_role(db.pool(), user.id, "admin").await?;
        println!("promoted {email} to administrator");
        return Ok(());
    }
    let password = std::env::var("RUSTY_FEED_ADMIN_PASSWORD")
        .map_err(|_| anyhow::anyhow!("RUSTY_FEED_ADMIN_PASSWORD must be set for a new account"))?;
    if password.chars().count() < 8 {
        anyhow::bail!("RUSTY_FEED_ADMIN_PASSWORD must contain at least 8 characters");
    }
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("password hashing failed: {error}"))?
        .to_string();
    let name = std::env::var("RUSTY_FEED_ADMIN_NAME").ok();
    rusty_feed_db::users::create(
        db.pool(),
        &email,
        name.as_deref(),
        None,
        "admin",
        Some(&password_hash),
    )
    .await?;
    println!("created administrator {email}");
    Ok(())
}

/// Queue one revision-keyed job per shared article missing the configured
/// vector space. Reindex first replaces the durable space guard and removes old
/// vectors, making the disruptive model transition explicit.
async fn queue_embedding_backfill(reindex: bool) -> anyhow::Result<()> {
    let config = rusty_feed_core::AppConfig::load()?;
    if !config.embeddings_enabled {
        anyhow::bail!(
            "embeddings are disabled (EMBEDDINGS_ENABLED=false); queuing jobs the worker will \
             never run would just fill the queue. Set EMBEDDINGS_ENABLED=true first."
        );
    }
    let model =
        rusty_feed_embed::canonical_model(config.embedding_provider, &config.embedding_model)?;
    let db = rusty_feed_db::Db::connect(&config.database_url).await?;
    db.migrate().await?;
    if reindex {
        let removed = rusty_feed_db::embedding_space::replace(
            db.pool(),
            &model,
            rusty_feed_embed::EMBEDDING_DIM as i32,
        )
        .await?;
        println!("removed {removed} vectors from the previous embedding space");
    } else {
        rusty_feed_db::embedding_space::ensure(
            db.pool(),
            &model,
            rusty_feed_embed::EMBEDDING_DIM as i32,
        )
        .await?;
    }

    let mut cursor = None;
    let mut queued = 0_u64;
    loop {
        let batch =
            rusty_feed_db::articles::list_needing_embedding_space(db.pool(), &model, cursor, 1_000)
                .await?;
        if batch.is_empty() {
            break;
        }
        for article in &batch {
            let dedup = format!("embed:{}:{}", article.id, article.content_revision);
            if rusty_feed_db::jobs::enqueue(
                db.pool(),
                "embed_article",
                serde_json::json!({
                    "article_id": article.id,
                    "content_revision": article.content_revision,
                }),
                Some(&dedup),
                None,
                3,
            )
            .await?
            .is_some()
            {
                queued += 1;
            }
        }
        cursor = batch.last().map(|article| article.id);
    }
    println!(
        "queued {queued} shared article revision(s) for model {}",
        model
    );
    Ok(())
}

/// Validate all application configuration and print only a secret-safe summary.
fn config_check() -> anyhow::Result<()> {
    let config = rusty_feed_core::AppConfig::load()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&config.diagnostic_summary())?
    );
    Ok(())
}

/// Apply database migrations. Reads `DATABASE_URL` directly (migrate needs only
/// the DB, not the full app config).
async fn migrate() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;

    let db = rusty_feed_db::Db::connect(&database_url).await?;
    db.migrate().await?;
    println!("migrations applied");
    Ok(())
}

/// Map an `anyhow::Result` to a process exit code, printing errors to stderr.
fn run(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "{banner}\n\n\
         Rusty Feed admin/maintenance CLI\n\n\
         USAGE:\n    rusty-feed-cli <COMMAND>\n\n\
         COMMANDS:\n\
         \x20   migrate           Apply database migrations\n\
         \x20   config-check      Validate config and print a secret-safe summary\n\
         \x20   create-admin      Create/promote using RUSTY_FEED_ADMIN_* environment variables\n\
         \x20   fetch-model       Download/cache and verify the embedding model\n\
         \x20   backfill          Queue missing vectors for the active model\n\
         \x20   canonicalize-urls [--apply]\n\
         \x20                      Dry-run or apply collision-free legacy URL cleanup\n\
         \x20   reindex --confirm Replace the vector space and queue every article\n\
         \x20   demo-reset --confirm\n\
         \x20                      Delete all demo accounts and their data\n\
         \x20   help              Show this help\n",
        banner = rusty_feed_core::banner()
    );
}
