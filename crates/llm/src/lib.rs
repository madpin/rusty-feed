//! # rustss-llm
//!
//! LLM integration behind the `LlmClient` port: OpenAI-compatible and Ollama
//! providers with a user -> admin -> env cascade, article summarization
//! (summary/key points/topics), and durable cost accounting (`cost_ledger`).
//!
//! Implemented in milestone **M12** (see `docs/ARCHITECTURE.md` §14).
