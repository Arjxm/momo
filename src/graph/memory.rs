//! Memory extraction after each agent interaction.
//! Uses the configured LLM provider to extract facts from conversations.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::graph::GraphBrain;
use crate::providers::{LLMProvider, Message};
use crate::types::{AgentError, MemoryNode, MemoryType};

/// Extracted memory from an interaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemory {
    pub content: String,
    pub memory_type: String,
    pub importance: f64,
    pub topics: Vec<String>,
}

/// Memory extractor that uses the configured LLM provider to extract facts from conversations
pub struct MemoryExtractor {
    provider: Arc<dyn LLMProvider>,
}

impl MemoryExtractor {
    /// Create a new memory extractor with the given LLM provider
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }
}

impl Clone for MemoryExtractor {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
        }
    }
}

impl MemoryExtractor {
    /// Extract memories from an interaction and store them in the graph
    pub async fn extract_and_store(
        &self,
        brain: &Arc<GraphBrain>,
        user_id: &str,
        user_input: &str,
        agent_response: &str,
        tools_used: &[String],
        episode_id: &str,
    ) -> Result<Vec<MemoryNode>, AgentError> {
        info!("🧠 [MEMORY] Starting memory extraction from interaction");
        debug!("🧠 [MEMORY] User input: {}", truncate(user_input, 100));
        debug!("🧠 [MEMORY] Tools used: {:?}", tools_used);

        // Build the extraction prompt
        let prompt = build_extraction_prompt(user_input, agent_response, tools_used);

        // Call LLM provider to extract memories
        info!("🧠 [MEMORY] Calling LLM to extract facts/preferences...");
        let messages = vec![Message::user(&prompt)];
        let response = self.provider.chat(&messages, &[], None).await?;

        // Parse the JSON response
        let extracted = parse_extraction_response(&response.text())?;

        if extracted.is_empty() {
            info!("🧠 [MEMORY] No new memories extracted from this interaction");
            return Ok(Vec::new());
        }

        info!("🧠 [MEMORY] Extracted {} memories from interaction:", extracted.len());
        for (i, mem) in extracted.iter().enumerate() {
            info!("🧠 [MEMORY]   {}. [{}] (importance: {:.2}) \"{}\"",
                i + 1, mem.memory_type, mem.importance, truncate(&mem.content, 60));
            debug!("🧠 [MEMORY]      Topics: {:?}", mem.topics);
        }

        // Store each extracted memory
        let mut stored_memories = Vec::new();

        for ext in extracted {
            // Convert memory type
            let memory_type = match ext.memory_type.as_str() {
                "preference" => MemoryType::Preference,
                "fact" => MemoryType::Fact,
                _ => MemoryType::Fact,
            };

            // Check for contradictions
            debug!("🧠 [MEMORY] Checking for contradictions with topics: {:?}", ext.topics);
            let contradictions = find_contradictions(brain, &ext.topics, &memory_type)?;

            // Create the memory node with provenance
            // Note: episode_id can be used as task_id for now
            let memory = MemoryNode::with_provenance(
                ext.content.clone(),
                memory_type.clone(),
                ext.importance,
                Some(episode_id.to_string()), // source_task_id
                None, // source_operation_id (could be enhanced later)
            );

            // Handle contradictions
            for old_memory in contradictions {
                if memories_contradict(&old_memory.content, &memory.content) {
                    warn!(
                        "🧠 [MEMORY] ⚠️ CONTRADICTION DETECTED:\n   OLD: \"{}\"\n   NEW: \"{}\"",
                        old_memory.content, memory.content
                    );

                    // Invalidate old memory
                    brain.invalidate_memory(&old_memory.id)?;
                    info!("🧠 [MEMORY] Invalidated old memory: {}", old_memory.id);

                    // Create contradiction and supersedes edges
                    brain.link_contradiction(&memory.id, &old_memory.id)?;
                    brain.link_supersedes(&memory.id, &old_memory.id)?;
                }
            }

            // Store the new memory with deduplication
            let (stored_id, was_duplicate) = brain.remember_with_dedup(&memory, &ext.topics)?;
            if was_duplicate {
                info!("🧠 [MEMORY] 🔄 Existing memory found: \"{}\" (id: {})", truncate(&memory.content, 50), &stored_id[..8]);
            } else {
                info!("🧠 [MEMORY] ✅ Stored new memory: \"{}\" (id: {})", truncate(&memory.content, 50), &stored_id[..8]);
            }

            // Link to episode
            link_memory_to_episode(brain, &memory.id, episode_id)?;

            // If it's a preference, link to user
            if memory_type == MemoryType::Preference {
                brain.link_user_preference(user_id, &memory.id)?;
                debug!("🧠 [MEMORY] Linked preference to user: {}", user_id);
            }

            stored_memories.push(memory);
        }

        info!("🧠 [MEMORY] Memory extraction complete. Stored {} new memories.", stored_memories.len());
        Ok(stored_memories)
    }
}

/// Build the extraction prompt for Claude Haiku
fn build_extraction_prompt(user_input: &str, agent_response: &str, tools_used: &[String]) -> String {
    let tools_str = if tools_used.is_empty() {
        "None".to_string()
    } else {
        tools_used.join(", ")
    };

    format!(
        r#"Extract facts, preferences, and observations from this interaction.
Return a JSON array. Each item should have:
{{
  "content": "the fact as a short sentence",
  "memory_type": "fact" or "preference",
  "importance": 0.0 to 1.0,
  "topics": ["topic1", "topic2"]
}}

Rules:
- Only extract genuinely new, specific information
- Skip obvious things or generic statements
- "preference" is for things the user explicitly prefers, likes, or wants
- "fact" is for information about the user, their work, or context
- importance: 0.9+ for critical info (name, job), 0.5-0.8 for useful context, below 0.5 for minor details
- topics should be 1-3 relevant keywords

If there's nothing notable to extract, return an empty array: []

User said: {user_input}
Agent responded: {agent_response}
Tools used: {tools_str}"#,
        user_input = user_input,
        agent_response = truncate(agent_response, 500),
        tools_str = tools_str
    )
}

/// Parse the extraction response from Claude
fn parse_extraction_response(response: &str) -> Result<Vec<ExtractedMemory>, AgentError> {
    // Find JSON array in the response
    let json_start = response.find('[');
    let json_end = response.rfind(']');

    match (json_start, json_end) {
        (Some(start), Some(end)) if start < end => {
            let json_str = &response[start..=end];
            serde_json::from_str(json_str).map_err(|e| {
                warn!("Failed to parse memory extraction JSON: {}", e);
                AgentError::ParseError(format!("Failed to parse memory extraction: {}", e))
            })
        }
        _ => {
            debug!("No JSON array found in extraction response");
            Ok(Vec::new())
        }
    }
}

/// Find potential contradicting memories in the graph
fn find_contradictions(
    brain: &Arc<GraphBrain>,
    topics: &[String],
    memory_type: &MemoryType,
) -> Result<Vec<MemoryNode>, AgentError> {
    if topics.is_empty() {
        return Ok(Vec::new());
    }

    // Only check for contradictions with preferences and facts
    let type_str = match memory_type {
        MemoryType::Preference => "preference",
        MemoryType::Fact => "fact",
        _ => return Ok(Vec::new()),
    };

    // Search for existing memories about the same topics
    let keywords: Vec<String> = topics.iter().cloned().collect();
    let existing = brain.recall(&keywords, 10)?;

    // Filter by memory type
    Ok(existing
        .into_iter()
        .filter(|m| m.memory_type.to_string() == type_str)
        .collect())
}

/// Simple heuristic to check if two memories contradict
fn memories_contradict(old: &str, new: &str) -> bool {
    let old_lower = old.to_lowercase();
    let new_lower = new.to_lowercase();

    // Check for direct negation patterns
    let negation_pairs = [
        ("prefer", "not prefer"),
        ("like", "dislike"),
        ("want", "don't want"),
        ("prefer concise", "prefer detailed"),
        ("prefer short", "prefer long"),
        ("prefer brief", "prefer comprehensive"),
    ];

    for (positive, negative) in negation_pairs {
        if (old_lower.contains(positive) && new_lower.contains(negative))
            || (old_lower.contains(negative) && new_lower.contains(positive))
        {
            return true;
        }
    }

    // Check if both are about the same subject but with different values
    // This is a simple heuristic - a real implementation might use semantic similarity
    let subjects = ["name is", "prefer", "like", "work at", "live in", "interested in"];

    for subject in subjects {
        if old_lower.contains(subject) && new_lower.contains(subject) {
            // Both mention the same subject - likely a contradiction if values differ
            // This is a simplification - real impl would extract and compare values
            if old_lower != new_lower {
                return true;
            }
        }
    }

    false
}

/// Link a memory to an episode
fn link_memory_to_episode(
    brain: &Arc<GraphBrain>,
    memory_id: &str,
    episode_id: &str,
) -> Result<(), AgentError> {
    // This would be a LEARNED_FROM edge
    // For now, we just log it since the schema supports it
    debug!("Memory {} learned from episode {}", memory_id, episode_id);
    Ok(())
}

/// Truncate text to a maximum length
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len]
    }
}

/// Record tool compositions discovered during an interaction
pub fn record_tool_compositions(
    brain: &Arc<GraphBrain>,
    tools_used: &[String],
) -> Result<(), AgentError> {
    if tools_used.len() < 2 {
        return Ok(());
    }

    // Record compositions between consecutive tools
    for window in tools_used.windows(2) {
        let from = &window[0];
        let to = &window[1];

        // Create composition edge with a generic description
        let desc = format!("{} followed by {}", from, to);
        brain.record_composition(from, to, &desc)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extraction_response() {
        let response = r#"Here are the extracted memories:
[
  {"content": "User's name is Alice", "memory_type": "fact", "importance": 0.9, "topics": ["personal", "name"]},
  {"content": "User prefers concise responses", "memory_type": "preference", "importance": 0.7, "topics": ["communication"]}
]"#;

        let extracted = parse_extraction_response(response).unwrap();
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].content, "User's name is Alice");
        assert_eq!(extracted[1].memory_type, "preference");
    }

    #[test]
    fn test_memories_contradict() {
        assert!(memories_contradict(
            "User prefers concise responses",
            "User prefers detailed responses"
        ));
        assert!(memories_contradict(
            "User likes dark mode",
            "User dislikes dark mode"
        ));
        assert!(!memories_contradict(
            "User works at Google",
            "User is interested in AI"
        ));
    }

    #[test]
    fn test_parse_empty_response() {
        let response = "No new information to extract. []";
        let extracted = parse_extraction_response(response).unwrap();
        assert!(extracted.is_empty());
    }
}
