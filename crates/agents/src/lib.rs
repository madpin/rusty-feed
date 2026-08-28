//! # rustss-agents
//!
//! The AGENTS framework: the `Agent` trait, scheduler (cron), durable Postgres
//! job queue, singleton leases, run records, and the outbox dispatcher — plus
//! the concrete agents (FeedRefresh, FetchFeed, ExtractContent, EmbedArticle,
//! SavedSearchMatch, Summarize, PatternDecay, Cleanup, ...).
//!
//! These agents are what guarantee O(1) shared work for N subscribers.
//! Framework implemented in milestone **M7** (see `docs/ARCHITECTURE.md` §8).
