//! # rusty-feed-embed
//!
//! One embedding space per instance (M9). An [`Embedder`] turns article text
//! into a fixed-`EMBEDDING_DIM` vector. Providers:
//! - **OpenAI-compatible** (`text-embedding-3-*` with `dimensions=384`), real.
//! - **Local** — BAAI BGE-small-en-v1.5 through `fastembed`/ONNX, downloaded
//!   once into the configured persistent cache and then usable offline.
//! - **Hash fallback** — available only when explicitly selecting the legacy
//!   `hash-placeholder-v1` model; it is never mixed with semantic vectors.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;

use rusty_feed_core::config::EmbeddingProvider;

/// The instance-wide embedding dimensionality (matches `article_embeddings`).
pub const EMBEDDING_DIM: usize = 384;

/// Turns text into fixed-dimension vectors.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Identifier stored alongside each vector (guards against mixing spaces).
    fn model(&self) -> &str;
    /// Provider label used for cost/usage accounting (e.g. `local`, `openai`).
    fn provider(&self) -> &str {
        "local"
    }
    fn dimensions(&self) -> usize;
    /// Embed a batch of texts (order preserved).
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;

    /// Convenience: embed a single text.
    async fn embed_one(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut out = self.embed(std::slice::from_ref(&text.to_string())).await?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("embedder returned no vector"))
    }
}

/// Build the article embedding input: `title\n\nexcerpt\n\ncontent[..2000]`.
pub fn prepare_text(title: &str, excerpt: Option<&str>, content: &str) -> String {
    let content: String = content.chars().take(2000).collect();
    format!("{}\n\n{}\n\n{}", title, excerpt.unwrap_or(""), content)
}

/// Configuration for [`build_embedder`].
#[derive(Debug, Clone)]
pub struct EmbedConfig {
    pub provider: EmbeddingProvider,
    pub model: String,
    pub openai_api_key: Option<String>,
    pub openai_base_url: Option<String>,
    pub model_cache_dir: PathBuf,
}

/// Normalize supported aliases before the durable space guard sees them. This
/// prevents a harmless spelling change from looking like a model migration.
pub fn canonical_model(provider: EmbeddingProvider, model: &str) -> anyhow::Result<String> {
    let model = model.trim();
    match provider {
        EmbeddingProvider::Local => match model {
            "BAAI/bge-small-en-v1.5" | "bge-small-en-v1.5" | "local-bge-small-en-v1.5" => {
                Ok("BAAI/bge-small-en-v1.5".to_string())
            }
            "hash-placeholder-v1" => Ok(model.to_string()),
            _ => anyhow::bail!(
                "unsupported local embedding model `{model}`; use BAAI/bge-small-en-v1.5"
            ),
        },
        EmbeddingProvider::Openai if model.is_empty() => {
            anyhow::bail!("EMBEDDING_MODEL must not be empty")
        }
        EmbeddingProvider::Openai => Ok(model.to_string()),
    }
}

/// Build the configured embedder.
pub fn build_embedder(config: EmbedConfig) -> anyhow::Result<Arc<dyn Embedder>> {
    let model = canonical_model(config.provider, &config.model)?;
    match config.provider {
        EmbeddingProvider::Openai => {
            let key = config.openai_api_key.ok_or_else(|| {
                anyhow::anyhow!("OPENAI_API_KEY required for the openai provider")
            })?;
            Ok(Arc::new(OpenAiEmbedder::new(
                model,
                key,
                config.openai_base_url,
            )))
        }
        EmbeddingProvider::Local if model == "hash-placeholder-v1" => {
            tracing::warn!(
                "legacy hash embedding model explicitly selected; semantic search is lexical"
            );
            Ok(Arc::new(HashEmbedder::default()))
        }
        EmbeddingProvider::Local => {
            Ok(Arc::new(LocalEmbedder::new(model, config.model_cache_dir)?))
        }
    }
}

/// Production on-device BGE embedder. Model initialization is lazy so the web
/// process can still serve RSS reading while a first download is unavailable;
/// `rusty-feed-cli fetch-model` performs the download deliberately for offline
/// deployments.
pub struct LocalEmbedder {
    model: String,
    cache_dir: PathBuf,
    inner: Arc<Mutex<Option<fastembed::TextEmbedding>>>,
}

impl LocalEmbedder {
    pub fn new(model: String, cache_dir: PathBuf) -> anyhow::Result<Self> {
        if model != "BAAI/bge-small-en-v1.5" {
            anyhow::bail!(
                "unsupported local embedding model `{model}`; use BAAI/bge-small-en-v1.5"
            );
        }
        Ok(Self {
            model,
            cache_dir,
            inner: Arc::new(Mutex::new(None)),
        })
    }

    fn initialize(cache_dir: PathBuf) -> anyhow::Result<fastembed::TextEmbedding> {
        std::fs::create_dir_all(&cache_dir).map_err(|error| {
            anyhow::anyhow!("cannot create model cache {}: {error}", cache_dir.display())
        })?;
        let options = fastembed::TextInitOptions::new(fastembed::EmbeddingModel::BGESmallENV15)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(false);
        fastembed::TextEmbedding::try_new(options).map_err(|error| {
            anyhow::anyhow!(
                "BGE model is unavailable: {error}. Run `rusty-feed-cli fetch-model` while online or seed MODEL_CACHE_DIR"
            )
        })
    }
}

#[async_trait]
impl Embedder for LocalEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }

    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inner = Arc::clone(&self.inner);
        let cache_dir = self.cache_dir.clone();
        let documents = texts.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut guard = inner
                .lock()
                .map_err(|_| anyhow::anyhow!("local embedding model lock was poisoned"))?;
            if guard.is_none() {
                *guard = Some(Self::initialize(cache_dir)?);
            }
            let model = guard
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("local embedding model failed to initialize"))?;
            let vectors = model.embed(documents, None)?;
            if vectors.iter().any(|vector| vector.len() != EMBEDDING_DIM) {
                anyhow::bail!("local embedding model returned an unexpected vector dimension");
            }
            Ok(vectors)
        })
        .await
        .map_err(|error| anyhow::anyhow!("local embedding task failed: {error}"))?
    }
}

/// Deterministic feature-hashing embedder (no model download). Good enough to
/// exercise the whole pipeline; similarity reflects lexical overlap.
pub struct HashEmbedder {
    model: String,
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self {
            model: "hash-placeholder-v1".to_string(),
        }
    }
}

impl HashEmbedder {
    fn embed_one(text: &str) -> Vec<f32> {
        let mut v = vec![0f32; EMBEDDING_DIM];
        for token in tokenize(text) {
            let h = fnv1a(token.as_bytes());
            let idx = (h % EMBEDDING_DIM as u64) as usize;
            let sign = if (h >> 40) & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
        l2_normalize(&mut v);
        v
    }
}

#[async_trait]
impl Embedder for HashEmbedder {
    fn model(&self) -> &str {
        &self.model
    }
    fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| Self::embed_one(t)).collect())
    }
}

/// OpenAI-compatible embeddings (`/v1/embeddings`, `dimensions=384`).
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    model: String,
    api_key: String,
    base_url: String,
}

impl OpenAiEmbedder {
    pub fn new(model: String, api_key: String, base_url: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            model,
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com".to_string()),
        }
    }
}

#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    fn model(&self) -> &str {
        &self.model
    }
    fn provider(&self) -> &str {
        "openai"
    }
    fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }
    async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = api_url(&self.base_url, "embeddings");
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "dimensions": EMBEDDING_DIM,
            "encoding_format": "float",
        });
        let resp = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let mut parsed: EmbeddingsResponse = resp.json().await?;
        parsed.data.sort_by_key(|d| d.index);
        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_ascii_lowercase())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn api_url(base_url: &str, endpoint: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/v1").unwrap_or(base);
    format!("{base}/v1/{endpoint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[tokio::test]
    async fn hash_embeddings_are_deterministic_and_normalized() {
        let e = HashEmbedder::default();
        let a = e.embed_one("Rust programming language").await.unwrap();
        let b = e.embed_one("Rust programming language").await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), EMBEDDING_DIM);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn hash_similarity_reflects_overlap() {
        let e = HashEmbedder::default();
        let q = e.embed_one("rust programming language").await.unwrap();
        let related = e
            .embed_one("the rust programming language guide")
            .await
            .unwrap();
        let unrelated = e.embed_one("italian pasta cooking recipes").await.unwrap();
        assert!(
            cosine(&q, &related) > cosine(&q, &unrelated),
            "shared words should score higher"
        );
    }

    #[tokio::test]
    async fn prepare_text_layout() {
        let t = prepare_text("Title", Some("Excerpt"), "Body");
        assert_eq!(t, "Title\n\nExcerpt\n\nBody");
    }

    #[test]
    fn accepts_openai_base_url_with_or_without_v1() {
        assert_eq!(
            api_url("https://api.openai.com", "embeddings"),
            "https://api.openai.com/v1/embeddings"
        );
        assert_eq!(
            api_url("https://api.openai.com/v1/", "embeddings"),
            "https://api.openai.com/v1/embeddings"
        );
    }

    #[test]
    fn local_model_aliases_share_one_durable_space_name() {
        for alias in [
            "BAAI/bge-small-en-v1.5",
            "bge-small-en-v1.5",
            "local-bge-small-en-v1.5",
        ] {
            assert_eq!(
                canonical_model(EmbeddingProvider::Local, alias).unwrap(),
                "BAAI/bge-small-en-v1.5"
            );
        }
        assert!(canonical_model(EmbeddingProvider::Local, "unknown").is_err());
    }
}
