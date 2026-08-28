//! RustSS admin/maintenance CLI.
//!
//! Milestone M0: command dispatch scaffold. Each subcommand is implemented in a
//! later milestone (see `docs/DEVELOPMENT_PLAN.md`): `migrate` (M2),
//! `create-admin` (M4), `fetch-model` (M9), `backfill`/`reindex` (M9),
//! `migrate-secrets` (M12). A richer parser (clap) is introduced when the first
//! command gains real flags.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str);

    match cmd {
        Some("migrate") => todo_cmd("migrate", "M2"),
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
         \x20   migrate           Apply database migrations (M2)\n\
         \x20   create-admin      Create/promote an admin user (M4)\n\
         \x20   fetch-model       Pre-download the embedding model (M9)\n\
         \x20   backfill          Backfill embeddings for existing articles (M9)\n\
         \x20   reindex           Rebuild vector/search indexes (M9)\n\
         \x20   migrate-secrets   Re-encrypt secrets under the unified AEAD (M12)\n\
         \x20   help              Show this help\n",
        banner = rustss_core::banner()
    );
}
