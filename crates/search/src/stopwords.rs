//! A compact English stop-word list (~130 terms) for keyword extraction.

use std::collections::HashSet;
use std::sync::LazyLock;

static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was",
        "one", "our", "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old",
        "see", "two", "way", "who", "boy", "did", "its", "let", "put", "say", "she", "too", "use",
        "that", "with", "have", "this", "will", "your", "from", "they", "know", "want", "been",
        "good", "much", "some", "time", "very", "when", "come", "here", "just", "like", "long",
        "make", "many", "over", "such", "take", "than", "them", "well", "were", "what", "would",
        "there", "their", "which", "about", "could", "other", "these", "thing", "think", "where",
        "being", "every", "great", "might", "shall", "still", "those", "under", "while", "should",
        "because", "before", "after", "between", "into", "through", "during", "without", "within",
        "along", "across", "behind", "beyond", "plus", "versus", "also", "back", "even", "then",
        "once", "only", "onto", "upon", "off", "per", "via", "yet", "she", "was", "does", "done",
        "each", "few", "had", "having", "http", "https", "www", "com", "org", "net", "html",
    ]
    .into_iter()
    .collect()
});

/// Whether `word` (already lowercased) is a stop word.
pub fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(word)
}
