//! RustSS admin/maintenance CLI.
//!
//! Implemented so far: `migrate` (M2). Remaining subcommands are milestone-tagged
//! stubs (see `docs/DEVELOPMENT_PLAN.md`): `create-admin` (M4), `fetch-model` (M9),
//! `backfill`/`reindex` (M9), `migrate-secrets` (M12). A richer parser (clap) is
//! introduced when the first command gains real flags.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("migrate") => run(migrate().await),
        Some("create-admin") => todo_cmd("create-admin", "M4"),
        Some("fetch-model") => todo_cmd("fetch-model", "M9"),
        Some("backfill") => todo_cmd("backfill", "M9"),
        Some("reindex") => todo_cmd("reindex", "M9"),
        Some("migrate-secrets") => todo_cmd("migrate-secrets", "M12"),
        Some("--version") | Some("-V") => {
            println!("{}", rustss_core::banner());
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

/// Apply database migrations. Reads `DATABASE_URL` directly (migrate needs only
/// the DB, not the full app config).
async fn migrate() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;

    let db = rustss_db::Db::connect(&database_url).await?;
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

fn todo_cmd(name: &str, milestone: &str) -> ExitCode {
    eprintln!("`{name}` is not implemented yet (scheduled for milestone {milestone}).");
    ExitCode::from(1)
}

fn print_help() {
    println!(
        "{banner}\n\n\
         RustSS admin/maintenance CLI\n\n\
         USAGE:\n    rustss-cli <COMMAND>\n\n\
         COMMANDS:\n\
         \x20   migrate           Apply database migrations\n\
         \x20   create-admin      Create/promote an admin user (M4)\n\
         \x20   fetch-model       Pre-download the embedding model (M9)\n\
         \x20   backfill          Backfill embeddings for existing articles (M9)\n\
         \x20   reindex           Rebuild vector/search indexes (M9)\n\
         \x20   migrate-secrets   Re-encrypt secrets under the unified AEAD (M12)\n\
         \x20   help              Show this help\n",
        banner = rustss_core::banner()
    );
}
