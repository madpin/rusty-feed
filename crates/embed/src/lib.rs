//! # rustss-embed
//!
//! Embedding generation behind the `Embedder` port: a local ONNX model
//! (`fastembed`, downloaded on first run) and an OpenAI-compatible provider.
//! Exactly **one embedding model per instance** — vectors are never padded or
//! truncated across dimensions (see `docs/FINDINGS.md` FND-002).
//!
//! Implemented in milestone **M9** (see `docs/ARCHITECTURE.md` §13).
