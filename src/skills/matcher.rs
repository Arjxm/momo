//! Skill matcher - matches user queries to relevant knowledge skills.

use std::collections::HashSet;

use tracing::debug;

use crate::skills::loader::SkillManifest;
use crate::skills::SkillType;

/// Maximum number of skills to inject per query
const MAX_MATCHED_SKILLS: usize = 2;

/// Maximum character length for skill content (to avoid context bloat)
const MAX_SKILL_CONTENT_LENGTH: usize = 8000;

/// Skill matcher that finds relevant knowledge skills for a given query
pub struct SkillMatcher;

impl SkillMatcher {
    /// Match user query against knowledge skills
    ///
    /// Returns skill manifests sorted by relevance (highest first), limited to MAX_MATCHED_SKILLS
    pub fn match_skills<'a>(
        query: &str,
        skills: &'a [&'a SkillManifest],
    ) -> Vec<&'a SkillManifest> {
        let query_lower = query.to_lowercase();

        // Extract significant words from query (filter out common short words)
        let query_words: HashSet<&str> = query_lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !is_stop_word(w))
            .collect();

        if query_words.is_empty() {
            return Vec::new();
        }

        // Score each knowledge skill
        let mut scored: Vec<(&SkillManifest, usize)> = skills
            .iter()
            .filter(|s| s.skill_type == SkillType::Knowledge)
            .map(|skill| {
                let score = calculate_match_score(&query_words, &query_lower, skill);
                (*skill, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect();

        // Sort by score (descending)
        scored.sort_by(|a, b| b.1.cmp(&a.1));

        // Take top matches
        let matched: Vec<_> = scored
            .into_iter()
            .take(MAX_MATCHED_SKILLS)
            .map(|(s, score)| {
                debug!("Matched skill '{}' with score {}", s.name, score);
                s
            })
            .collect();

        matched
    }

    /// Format matched skills for injection into system prompt
    pub fn format_for_prompt(skills: &[&SkillManifest]) -> String {
        if skills.is_empty() {
            return String::new();
        }

        let mut output = String::from("\n\n## Reference Documentation\n");
        output.push_str("The following documentation is provided to help answer the user's query:\n");

        for skill in skills {
            if let Some(content) = &skill.content {
                output.push_str("\n---\n### ");
                output.push_str(&skill.name);
                output.push_str("\n\n");

                // Truncate if too long
                if content.len() > MAX_SKILL_CONTENT_LENGTH {
                    output.push_str(&content[..MAX_SKILL_CONTENT_LENGTH]);
                    output.push_str("\n\n[Content truncated...]");
                } else {
                    output.push_str(content);
                }
            }
        }

        output
    }
}

/// Calculate match score for a skill based on keyword overlap
fn calculate_match_score(
    query_words: &HashSet<&str>,
    query_lower: &str,
    skill: &SkillManifest,
) -> usize {
    let mut score = 0;
    let desc_lower = skill.description.to_lowercase();
    let name_lower = skill.name.to_lowercase();

    // Exact name match gets highest score
    if query_lower.contains(&name_lower) || name_lower.contains(query_lower.trim()) {
        score += 10;
    }

    // Check description for keyword matches
    for word in query_words {
        if desc_lower.contains(word) {
            score += 2;
        }
        if name_lower.contains(word) {
            score += 3;
        }
    }

    // Check topics for matches
    for topic in &skill.topics {
        let topic_lower = topic.to_lowercase();
        for word in query_words {
            if topic_lower.contains(word) || word.contains(&topic_lower.as_str()) {
                score += 2;
            }
        }
    }

    // Check skill content for keyword presence (lower weight to avoid false positives)
    if let Some(content) = &skill.content {
        let content_lower = content.to_lowercase();
        let content_word_matches = query_words
            .iter()
            .filter(|word| content_lower.contains(*word))
            .count();
        // Only add to score if multiple words match (reduces noise)
        if content_word_matches >= 2 {
            score += content_word_matches;
        }
    }

    score
}

/// Common stop words to filter out
fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the" | "a" | "an" | "is" | "are" | "was" | "were" | "be" | "been"
            | "being" | "have" | "has" | "had" | "do" | "does" | "did"
            | "will" | "would" | "could" | "should" | "may" | "might"
            | "can" | "to" | "of" | "in" | "for" | "on" | "with" | "at"
            | "by" | "from" | "up" | "about" | "into" | "over" | "after"
            | "how" | "what" | "when" | "where" | "why" | "who" | "which"
            | "this" | "that" | "these" | "those" | "and" | "but" | "or"
            | "not" | "no" | "yes" | "all" | "any" | "some" | "just"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_test_skill(name: &str, description: &str, content: &str) -> SkillManifest {
        SkillManifest {
            name: name.to_string(),
            description: description.to_string(),
            version: 1,
            skill_type: SkillType::Knowledge,
            language: None,
            entrypoint: None,
            content: Some(content.to_string()),
            input_schema: serde_json::Value::Null,
            output_schema: serde_json::Value::Null,
            topics: vec![],
            composes_with: vec![],
        }
    }

    #[test]
    fn test_match_by_name() {
        let bun_skill = create_test_skill(
            "bun",
            "Build fast applications with Bun JavaScript runtime",
            "# Bun Documentation",
        );
        let react_skill = create_test_skill(
            "react",
            "React UI library for building components",
            "# React Documentation",
        );

        let skills: Vec<&SkillManifest> = vec![&bun_skill, &react_skill];

        let matches = SkillMatcher::match_skills("How do I create a Bun server?", &skills);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "bun");
    }

    #[test]
    fn test_match_by_description() {
        let docker_skill = create_test_skill(
            "docker",
            "Container runtime for building and deploying applications. Dockerfile, compose, images.",
            "# Docker Documentation",
        );
        let skill_ref = &docker_skill;
        let skills = vec![skill_ref];

        let matches = SkillMatcher::match_skills("How do I create a Dockerfile?", &skills);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "docker");
    }

    #[test]
    fn test_no_match_returns_empty() {
        let bun_skill = create_test_skill(
            "bun",
            "JavaScript runtime",
            "# Bun",
        );
        let skills = vec![&bun_skill];

        let matches = SkillMatcher::match_skills("How do I cook pasta?", &skills);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_max_skills_limit() {
        let skill1 = create_test_skill("rust", "Rust programming language", "# Rust");
        let skill2 = create_test_skill("cargo", "Rust package manager cargo", "# Cargo");
        let skill3 = create_test_skill("rustup", "Rust toolchain installer", "# Rustup");

        let skills: Vec<&SkillManifest> = vec![&skill1, &skill2, &skill3];

        let matches = SkillMatcher::match_skills("How do I use Rust cargo?", &skills);
        assert!(matches.len() <= MAX_MATCHED_SKILLS);
    }
}
