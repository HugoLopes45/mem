/// Pure-frequency suggest-rules engine — no LLM, no new deps.
///
/// Algorithm:
/// 1. Tokenize titles + content: lowercase, split on non-alphanumeric, filter short/stop-words
/// 2. Count per-token frequency across all memories
/// 3. Emit tokens appearing in >= 3 memories as rule candidates
/// 4. Also detect bigrams appearing in >= 2 memories for more specific rules
/// 5. Output CLAUDE.md-ready markdown
use crate::types::Memory;
use std::collections::HashMap;

const STOP_WORDS: &[&str] = &[
    // English stop words
    "the", "a", "an", "is", "was", "to", "in", "of", "and", "or", "with", "for", "on", "at", "be",
    "has", "have", "had", "by", "as", "this", "that", "it", "from", "are", "were", "not", "no",
    "so", "if", "but", "its", "via", "use", "used", "new", "get", "set", "run", "add", "fix",
    "now", "also", "just", "into", "than", "all", "any", "one", "two", "do", "done", "we", "my",
    "our", "you", "your", "will", "can", "may", "must", "then", "when", "where", "what", "how",
    "out", "up", "end", "been", "about", "more", "some", "such", "them", "they",
    // Domain noise: auto-capture boilerplate terms
    "session", "git", "ended", "changes", "detected", "captured", "utc", "project", "repo", "mem",
    "memory", "context", "2026", "00", "date", "time",
];

/// Tokenize a string into lowercase words >= 3 chars, filtering stop-words.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter_map(|tok| {
            let lower = tok.to_lowercase();
            if lower.len() >= 3 && !STOP_WORDS.contains(&lower.as_str()) {
                Some(lower)
            } else {
                None
            }
        })
        .collect()
}

/// Extract bigrams from a token list.
fn bigrams(tokens: &[String]) -> Vec<String> {
    tokens
        .windows(2)
        .map(|pair| format!("{} {}", pair[0], pair[1]))
        .collect()
}

/// Analyse memories and return CLAUDE.md-ready markdown suggestions.
pub fn suggest_rules(memories: &[Memory], analysed_limit: usize) -> String {
    let count = memories.len();

    // token → set of memory indices that contain it (for per-memory frequency)
    let mut token_memories: HashMap<String, Vec<usize>> = HashMap::new();
    let mut bigram_memories: HashMap<String, Vec<usize>> = HashMap::new();

    for (idx, mem) in memories.iter().enumerate() {
        let combined = format!("{} {}", mem.title, mem.content);
        let tokens = tokenize(&combined);
        let bgs = bigrams(&tokens);

        let mut seen_tokens = std::collections::HashSet::new();
        for tok in tokens {
            if seen_tokens.insert(tok.clone()) {
                token_memories.entry(tok).or_default().push(idx);
            }
        }

        let mut seen_bigrams = std::collections::HashSet::new();
        for bg in bgs {
            if seen_bigrams.insert(bg.clone()) {
                bigram_memories.entry(bg).or_default().push(idx);
            }
        }
    }

    // Collect candidates: token appears in >= 3 memories
    let mut unigram_candidates: Vec<(String, usize)> = token_memories
        .into_iter()
        .filter(|(_, idxs)| idxs.len() >= 3)
        .map(|(tok, idxs)| (tok, idxs.len()))
        .collect();
    unigram_candidates.sort_by(|a, b| b.1.cmp(&a.1));

    // Bigram candidates: bigram appears in >= 2 memories
    let mut bigram_candidates: Vec<(String, usize)> = bigram_memories
        .into_iter()
        .filter(|(_, idxs)| idxs.len() >= 2)
        .map(|(bg, idxs)| (bg, idxs.len()))
        .collect();
    bigram_candidates.sort_by(|a, b| b.1.cmp(&a.1));

    // Build output
    let today = chrono::Utc::now().format("%Y-%m-%d");
    let mut out = format!(
        "## Suggested rules (from mem pattern analysis)\n\
         <!-- based on {count} sessions, {today} (analysed last {analysed_limit} auto-captured memories) -->\n\n"
    );

    if unigram_candidates.is_empty() && bigram_candidates.is_empty() {
        out.push_str(
            "No recurring patterns detected yet. Capture more sessions for better suggestions.\n",
        );
        return out;
    }

    // Emit bigrams first (more specific)
    if !bigram_candidates.is_empty() {
        out.push_str("### Recurring phrase patterns\n\n");
        for (phrase, freq) in bigram_candidates.iter().take(10) {
            out.push_str(&format!(
                "- [detected phrase: \"{phrase}\" appears in {freq}x sessions] Consider adding a rule about: `{phrase}`\n"
            ));
        }
        out.push('\n');
    }

    // Emit unigrams
    if !unigram_candidates.is_empty() {
        out.push_str("### Recurring single-term patterns\n\n");
        for (term, freq) in unigram_candidates.iter().take(15) {
            out.push_str(&format!(
                "- [detected term: \"{term}\" appears in {freq}x sessions] Consider adding: \
                 \"This project uses/involves `{term}`\"\n"
            ));
        }
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Memory, MemoryScope, MemoryStatus, MemoryType};
    use chrono::Utc;

    fn make_memory(title: &str, content: &str) -> Memory {
        Memory {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: None,
            project: None,
            title: title.to_string(),
            memory_type: MemoryType::Auto,
            content: content.to_string(),
            git_diff: None,
            created_at: Utc::now(),
            access_count: 0,
            last_accessed_at: None,
            status: MemoryStatus::Active,
            scope: MemoryScope::Project,
        }
    }

    #[test]
    fn tokenize_filters_stop_words_and_short_tokens() {
        let tokens = tokenize("the JWT authentication is used for the auth flow");
        assert!(tokens.contains(&"jwt".to_string()));
        assert!(tokens.contains(&"authentication".to_string()));
        // Stop words and short tokens removed
        assert!(!tokens.contains(&"the".to_string()));
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"for".to_string()));
    }

    #[test]
    fn suggest_rules_detects_recurring_term() {
        // 3 memories all mentioning "jwt"
        let memories = vec![
            make_memory(
                "JWT auth setup",
                "Configured JWT token auth with 24h expiry",
            ),
            make_memory("JWT refresh flow", "JWT refresh tokens expire after 7 days"),
            make_memory(
                "session and JWT",
                "JWT is used throughout the auth pipeline",
            ),
        ];

        let output = suggest_rules(&memories, 20);
        assert!(
            output.contains("jwt"),
            "output should mention jwt: {output}"
        );
        assert!(output.contains("3x"), "should show frequency of 3");
    }

    #[test]
    fn suggest_rules_emits_markdown_header() {
        let memories = vec![make_memory("title a", "content a")];
        let output = suggest_rules(&memories, 20);
        assert!(output.contains("## Suggested rules"));
        assert!(output.contains("from mem pattern analysis"));
    }

    #[test]
    fn suggest_rules_reports_no_patterns_when_below_threshold() {
        // Each memory has unique tokens — no term appears >= 3 times
        let memories = vec![
            make_memory("alpha bravo charlie", "delta echo foxtrot"),
            make_memory("golf hotel india", "juliet kilo lima"),
        ];
        let output = suggest_rules(&memories, 20);
        assert!(output.contains("No recurring patterns"));
    }

    #[test]
    fn suggest_rules_detects_bigram() {
        let memories = vec![
            make_memory("tokio runtime setup", "configured tokio runtime async"),
            make_memory("tokio runtime config", "tokio runtime handles threads"),
        ];
        let output = suggest_rules(&memories, 20);
        // "tokio runtime" bigram should appear (2 memories)
        assert!(
            output.contains("tokio runtime"),
            "bigram should be detected"
        );
    }

    #[test]
    fn tokenize_respects_min_length_boundary() {
        let tokens = tokenize("db api jwt");
        assert!(
            !tokens.contains(&"db".to_string()),
            "2-char token must be filtered"
        );
        assert!(
            tokens.contains(&"api".to_string()),
            "3-char token must survive"
        );
    }

    #[test]
    fn suggest_rules_suppresses_bigram_with_only_one_occurrence() {
        let memories = vec![make_memory(
            "tokio runtime setup",
            "tokio runtime handles async",
        )];
        let output = suggest_rules(&memories, 20);
        assert!(
            !output.contains("tokio runtime"),
            "bigram in only 1 memory must not be suggested"
        );
    }
}
