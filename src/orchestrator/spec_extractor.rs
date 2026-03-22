//! Specification Extractor - Extracts validation requirements from task descriptions.
//!
//! Uses LLM to parse natural language task descriptions and extract:
//! - Numeric requirements (counts, quantities)
//! - Expected outputs (files, data, artifacts)
//! - Qualitative requirements (quality, format specifications)
//! - Keywords for similarity matching

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::providers::{LLMProvider, Message};
use crate::types::AgentError;
use super::types::{
    TaskSpecification, NumericRequirement, ExpectedOutput,
    ComparisonOp, OutputType,
};

/// Extracts task specifications from natural language descriptions
#[derive(Clone)]
pub struct SpecExtractor {
    provider: Arc<dyn LLMProvider>,
}

impl SpecExtractor {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    /// Extract specification from a task description
    pub async fn extract(&self, task_description: &str) -> Result<TaskSpecification, AgentError> {
        info!("[SPEC] Extracting specification from: \"{}\"",
            if task_description.len() > 60 { &task_description[..60] } else { task_description });

        let prompt = self.build_extraction_prompt(task_description);
        let messages = vec![Message::user(&prompt)];

        let response = self.provider.chat(&messages, &[], Some(SYSTEM_PROMPT)).await?;
        let response_text = response.text();

        debug!("[SPEC] LLM response: {}", &response_text);

        self.parse_response(&response_text, task_description)
    }

    /// Build the extraction prompt
    fn build_extraction_prompt(&self, task_description: &str) -> String {
        format!(
            r#"Analyze this task and extract validation requirements:

TASK: {}

Respond with JSON only (no markdown):
{{
  "numeric_requirements": [
    {{"entity": "what is being counted", "expected_count": N, "comparison": "exactly|at_least|at_most"}}
  ],
  "expected_outputs": [
    {{"name": "output name/pattern", "output_type": "file|data|message|artifact", "required": true|false}}
  ],
  "qualitative_requirements": [
    "requirement description"
  ],
  "keywords": ["keyword1", "keyword2"]
}}

Examples:
- "Search 3 websites" -> numeric: {{"entity": "websites", "expected_count": 3, "comparison": "exactly"}}
- "Extract at least 10 products" -> numeric: {{"entity": "products", "expected_count": 10, "comparison": "at_least"}}
- "Save to CSV file" -> output: {{"name": "*.csv", "output_type": "file", "required": true}}
- "Compare prices" -> qualitative: "Must compare prices across sources"

Extract ALL requirements from the task. Be thorough."#,
            task_description
        )
    }

    /// Parse the LLM response into a TaskSpecification
    fn parse_response(&self, response: &str, original_description: &str) -> Result<TaskSpecification, AgentError> {
        // Try to extract JSON from the response
        let json_str = extract_json(response);

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| {
                warn!("[SPEC] Failed to parse JSON: {}, using fallback", e);
                AgentError::ParseError(format!("Failed to parse spec JSON: {}", e))
            })?;

        let mut spec = TaskSpecification::new(original_description.to_string());

        // Parse numeric requirements
        if let Some(nums) = parsed.get("numeric_requirements").and_then(|v| v.as_array()) {
            for num in nums {
                if let Some(req) = parse_numeric_requirement(num) {
                    spec.numeric_requirements.push(req);
                }
            }
        }

        // Parse expected outputs
        if let Some(outputs) = parsed.get("expected_outputs").and_then(|v| v.as_array()) {
            for out in outputs {
                if let Some(exp) = parse_expected_output(out) {
                    spec.expected_outputs.push(exp);
                }
            }
        }

        // Parse qualitative requirements
        if let Some(quals) = parsed.get("qualitative_requirements").and_then(|v| v.as_array()) {
            for qual in quals {
                if let Some(s) = qual.as_str() {
                    spec.qualitative_requirements.push(s.to_string());
                }
            }
        }

        // Parse keywords
        if let Some(keywords) = parsed.get("keywords").and_then(|v| v.as_array()) {
            for kw in keywords {
                if let Some(s) = kw.as_str() {
                    spec.keywords.push(s.to_lowercase());
                }
            }
        }

        // If no keywords extracted, generate from description
        if spec.keywords.is_empty() {
            spec.keywords = extract_keywords_from_description(original_description);
        }

        info!("[SPEC] Extracted: {} numeric reqs, {} outputs, {} qualitative, {} keywords",
            spec.numeric_requirements.len(),
            spec.expected_outputs.len(),
            spec.qualitative_requirements.len(),
            spec.keywords.len()
        );

        Ok(spec)
    }

    /// Quick extraction without LLM (regex-based) for simple cases
    pub fn extract_quick(&self, task_description: &str) -> TaskSpecification {
        let mut spec = TaskSpecification::new(task_description.to_string());

        // Extract numbers with context (e.g., "3 sites", "10 products")
        let re = regex::Regex::new(r"(\d+)\s+([a-zA-Z]+)").unwrap();
        for cap in re.captures_iter(task_description) {
            if let (Some(count), Some(entity)) = (cap.get(1), cap.get(2)) {
                if let Ok(n) = count.as_str().parse::<u32>() {
                    let entity_str = entity.as_str().to_lowercase();
                    // Skip common non-requirement numbers
                    if !["minutes", "seconds", "hours", "days", "times", "steps"].contains(&entity_str.as_str()) {
                        spec.numeric_requirements.push(NumericRequirement {
                            entity: entity_str,
                            expected_count: n,
                            comparison: ComparisonOp::AtLeast,
                        });
                    }
                }
            }
        }

        // Extract file outputs
        let file_patterns = [
            (r"save\s+(?:to\s+)?(\w+\.(?:csv|json|txt|md|html))", true),
            (r"output\s+(?:to\s+)?(\w+\.(?:csv|json|txt|md|html))", true),
            (r"create\s+(?:a\s+)?(\w+\.(?:csv|json|txt|md|html))", true),
            (r"(?:csv|json|txt)\s+file", false),
        ];

        for (pattern, has_name) in file_patterns {
            let re = regex::Regex::new(pattern).unwrap();
            if has_name {
                for cap in re.captures_iter(&task_description.to_lowercase()) {
                    if let Some(name) = cap.get(1) {
                        spec.expected_outputs.push(ExpectedOutput {
                            name: name.as_str().to_string(),
                            output_type: OutputType::File,
                            required: true,
                        });
                    }
                }
            } else if re.is_match(&task_description.to_lowercase()) {
                spec.expected_outputs.push(ExpectedOutput {
                    name: "*.csv|*.json|*.txt".to_string(),
                    output_type: OutputType::File,
                    required: true,
                });
            }
        }

        // Extract keywords
        spec.keywords = extract_keywords_from_description(task_description);

        spec
    }
}

/// System prompt for specification extraction
const SYSTEM_PROMPT: &str = r#"You are a task specification analyzer. Your job is to extract concrete, testable requirements from task descriptions.

Focus on:
1. NUMERIC: Exact counts or minimums (e.g., "3 sites" = exactly 3, "at least 5" = minimum 5)
2. OUTPUTS: Files or artifacts that should be created (CSV, JSON, reports, etc.)
3. QUALITATIVE: Quality or format requirements (comparisons, sorting, filtering)
4. KEYWORDS: Key terms for matching similar tasks

Be precise and err on the side of extracting more requirements rather than fewer.
Respond with valid JSON only, no explanations."#;

/// Extract JSON from a response that might contain markdown or other text
fn extract_json(response: &str) -> String {
    // Try to find JSON block in markdown
    if let Some(start) = response.find("```json") {
        if let Some(end) = response[start + 7..].find("```") {
            return response[start + 7..start + 7 + end].trim().to_string();
        }
    }

    // Try to find JSON block without language specifier
    if let Some(start) = response.find("```") {
        let after_start = &response[start + 3..];
        if let Some(end) = after_start.find("```") {
            let content = after_start[..end].trim();
            if content.starts_with('{') {
                return content.to_string();
            }
        }
    }

    // Try to find raw JSON object
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            return response[start..=end].to_string();
        }
    }

    response.to_string()
}

/// Parse a numeric requirement from JSON
fn parse_numeric_requirement(value: &serde_json::Value) -> Option<NumericRequirement> {
    let entity = value.get("entity")?.as_str()?.to_string();
    let expected_count = value.get("expected_count")?.as_u64()? as u32;
    let comparison = match value.get("comparison").and_then(|v| v.as_str()) {
        Some("exactly") => ComparisonOp::Exactly,
        Some("at_most") => ComparisonOp::AtMost,
        _ => ComparisonOp::AtLeast,
    };

    Some(NumericRequirement {
        entity,
        expected_count,
        comparison,
    })
}

/// Parse an expected output from JSON
fn parse_expected_output(value: &serde_json::Value) -> Option<ExpectedOutput> {
    let name = value.get("name")?.as_str()?.to_string();
    let output_type = match value.get("output_type").and_then(|v| v.as_str()) {
        Some("file") => OutputType::File,
        Some("data") => OutputType::Data,
        Some("message") => OutputType::Message,
        Some("artifact") => OutputType::Artifact,
        _ => OutputType::Data,
    };
    let required = value.get("required").and_then(|v| v.as_bool()).unwrap_or(true);

    Some(ExpectedOutput {
        name,
        output_type,
        required,
    })
}

/// Extract keywords from a task description
fn extract_keywords_from_description(description: &str) -> Vec<String> {
    let stop_words = [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "must", "shall", "can", "need", "to", "of",
        "in", "for", "on", "with", "at", "by", "from", "as", "into", "and",
        "or", "but", "if", "then", "than", "so", "that", "this", "these",
        "those", "it", "its", "i", "you", "we", "they", "my", "your", "our",
        "their", "me", "him", "her", "us", "them", "all", "each", "every",
        "some", "any", "no", "not", "only", "just", "also", "very", "too",
    ];

    description
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| {
            let word = word.trim();
            word.len() > 2 && !stop_words.contains(&word)
        })
        .take(10)
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_keywords() {
        let desc = "Search 3 e-commerce sites for headphones and compare prices";
        let keywords = extract_keywords_from_description(desc);
        assert!(keywords.contains(&"search".to_string()));
        assert!(keywords.contains(&"headphones".to_string()));
        assert!(keywords.contains(&"prices".to_string()));
    }

    #[test]
    fn test_extract_json() {
        let response = r#"Here's the analysis:
```json
{"numeric_requirements": []}
```
Done!"#;
        let json = extract_json(response);
        assert!(json.starts_with('{'));
        assert!(json.contains("numeric_requirements"));
    }
}
