//! Saved-search query DSL (M11).
//!
//! Grammar (ARCH §11.4): comma = OR, `+` = required, `-` = excluded,
//! `"..."` = phrase, `(...)` = group. Plain terms are required (AND); the only
//! disjunction is the comma (and groups). Parsing is **total** — any input
//! yields a `Query` (never panics), which the property tests assert.
//!
//! ```text
//! AI                 → term "ai"
//! AI, machine        → (ai) OR (machine)
//! AI +ethics         → ai AND ethics
//! AI -crypto         → ai AND NOT crypto
//! "machine learning" → phrase
//! (AI, ML) +ethics   → (ai OR ml) AND ethics
//! ```

use std::collections::HashSet;

/// Whether a clause must or must-not match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occur {
    Must,
    MustNot,
}

/// A leaf or sub-expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Term(String),
    Phrase(String),
    Group(Query),
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    /// Disjunction (comma / group alternatives).
    Or(Vec<Query>),
    /// A conjunction of occurrence-tagged nodes.
    Seq(Vec<(Occur, Node)>),
}

/// Parse a query string into a [`Query`]. Total: never panics.
pub fn parse(input: &str) -> Query {
    Parser {
        chars: input.chars().collect(),
        pos: 0,
    }
    .parse_or_list()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn parse_or_list(&mut self) -> Query {
        let mut alts = vec![self.parse_seq()];
        loop {
            self.skip_ws();
            if self.peek() == Some(',') {
                self.advance();
                alts.push(self.parse_seq());
            } else {
                break;
            }
        }
        if alts.len() == 1 {
            alts.pop().unwrap()
        } else {
            Query::Or(alts)
        }
    }

    fn parse_seq(&mut self) -> Query {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None | Some(',') | Some(')') => break,
                _ => {
                    if let Some(item) = self.parse_factor() {
                        items.push(item);
                    }
                }
            }
        }
        Query::Seq(items)
    }

    fn parse_factor(&mut self) -> Option<(Occur, Node)> {
        self.skip_ws();
        let occur = match self.peek() {
            Some('+') => {
                self.advance();
                Occur::Must
            }
            Some('-') => {
                self.advance();
                Occur::MustNot
            }
            _ => Occur::Must,
        };
        self.parse_atom().map(|node| (occur, node))
    }

    fn parse_atom(&mut self) -> Option<Node> {
        match self.peek() {
            Some('"') => {
                self.advance();
                let mut phrase = String::new();
                while let Some(c) = self.peek() {
                    if c == '"' {
                        self.advance();
                        break;
                    }
                    phrase.push(c);
                    self.advance();
                }
                let phrase = phrase.trim().to_ascii_lowercase();
                (!phrase.is_empty()).then_some(Node::Phrase(phrase))
            }
            Some('(') => {
                self.advance();
                let inner = self.parse_or_list();
                self.skip_ws();
                if self.peek() == Some(')') {
                    self.advance();
                }
                Some(Node::Group(inner))
            }
            _ => {
                let mut word = String::new();
                while let Some(c) = self.peek() {
                    if c.is_whitespace() || matches!(c, ',' | '(' | ')' | '"') {
                        break;
                    }
                    word.push(c);
                    self.advance();
                }
                let word = word.trim().to_ascii_lowercase();
                (!word.is_empty()).then_some(Node::Term(word))
            }
        }
    }
}

impl Query {
    /// Boolean match against an article's token set and lowercased text.
    pub fn matches(&self, tokens: &HashSet<String>, text_lower: &str) -> bool {
        match self {
            Query::Or(alts) => alts.iter().any(|q| q.matches(tokens, text_lower)),
            Query::Seq(items) => {
                if items.is_empty() {
                    return false;
                }
                for (occur, node) in items {
                    let m = node.node_matches(tokens, text_lower);
                    match occur {
                        Occur::Must if !m => return false,
                        Occur::MustNot if m => return false,
                        _ => {}
                    }
                }
                true
            }
        }
    }

    /// Positive (non-excluded) terms/phrases, for keyword scoring and building
    /// the query's embedding text.
    pub fn positive_terms(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_positive(&mut out);
        out
    }

    fn collect_positive(&self, out: &mut Vec<String>) {
        match self {
            Query::Or(alts) => alts.iter().for_each(|q| q.collect_positive(out)),
            Query::Seq(items) => {
                for (occur, node) in items {
                    if *occur == Occur::MustNot {
                        continue;
                    }
                    match node {
                        Node::Term(t) | Node::Phrase(t) => out.push(t.clone()),
                        Node::Group(q) => q.collect_positive(out),
                    }
                }
            }
        }
    }
}

impl Node {
    fn node_matches(&self, tokens: &HashSet<String>, text_lower: &str) -> bool {
        match self {
            Node::Term(t) => tokens.contains(t),
            Node::Phrase(p) => text_lower.contains(p),
            Node::Group(q) => q.matches(tokens, text_lower),
        }
    }
}

/// Tokenize article text into a lowercase word set (for boolean matching).
pub fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_examples() {
        assert_eq!(
            parse("AI"),
            Query::Seq(vec![(Occur::Must, Node::Term("ai".into()))])
        );
        assert!(matches!(parse("AI, ML"), Query::Or(v) if v.len() == 2));
        let q = parse("AI +ethics -crypto");
        let Query::Seq(items) = &q else { panic!() };
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].0, Occur::MustNot);
    }

    #[test]
    fn boolean_semantics() {
        let text = "ai and ethics in society";
        let t = tokenize(text);

        assert!(parse("ai").matches(&t, text));
        assert!(parse("ai, missing").matches(&t, text)); // OR
        assert!(parse("ai +ethics").matches(&t, text)); // AND both present
        assert!(!parse("ai +crypto").matches(&t, text)); // required missing
        assert!(!parse("ai -ethics").matches(&t, text)); // excluded present
        assert!(parse("ai -crypto").matches(&t, text)); // excluded absent
        assert!(parse("(ai, ml) +ethics").matches(&t, text)); // group OR + required
        assert!(!parse("(x, y) +ethics").matches(&t, text)); // group fails
    }

    #[test]
    fn phrase_matches_substring() {
        let text = "a study of machine learning models";
        let t = tokenize(text);
        assert!(parse("\"machine learning\"").matches(&t, text));
        assert!(!parse("\"learning machine\"").matches(&t, text));
    }

    #[test]
    fn positive_terms_exclude_negations() {
        let terms = parse("ai +ethics -crypto \"deep learning\"").positive_terms();
        assert!(terms.contains(&"ai".to_string()));
        assert!(terms.contains(&"ethics".to_string()));
        assert!(terms.contains(&"deep learning".to_string()));
        assert!(!terms.contains(&"crypto".to_string()));
    }

    proptest::proptest! {
        #[test]
        fn parsing_never_panics(s in ".{0,64}") {
            let _ = parse(&s); // must not panic on arbitrary input
        }

        #[test]
        fn parsing_operator_soup_is_total(
            s in "[a-z0-9 ,+\\-()\"]{0,48}"
        ) {
            let q = parse(&s);
            let t = tokenize("alpha beta gamma");
            let _ = q.matches(&t, "alpha beta gamma"); // evaluation is total too
        }
    }
}
