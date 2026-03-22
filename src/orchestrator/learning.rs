//! Learning Integration - Builds mistake context for prompts and correction instructions.
//!
//! Provides:
//! - Building mistake context for system prompts
//! - Creating correction prompts for retries
//! - Formatting mistakes for different contexts

use std::sync::Arc;
use tracing::{debug, info};

use crate::graph::GraphBrain;
use crate::types::{AgentError, MistakeNode, Severity};
use super::types::{TaskSpecification, ValidationResult};

/// Learning module for building mistake-aware prompts
pub struct LearningModule {
    brain: Arc<GraphBrain>,
}

impl LearningModule {
    pub fn new(brain: Arc<GraphBrain>) -> Self {
        Self { brain }
    }

    /// Build mistake context to inject into system prompt
    ///
    /// Returns a formatted string with relevant past mistakes and prevention strategies
    pub fn build_mistake_context(
        &self,
        task_description: &str,
        spec: Option<&TaskSpecification>,
    ) -> Result<String, AgentError> {
        // Extract keywords from task or spec
        let keywords: Vec<String> = if let Some(s) = spec {
            s.keywords.clone()
        } else {
            extract_keywords(task_description)
        };

        // Get fingerprint if available
        let fingerprint = spec.map(|s| s.fingerprint()).unwrap_or_default();

        // Recall relevant mistakes
        let mistakes = self.brain.recall_relevant_mistakes(&keywords, &fingerprint, 5)?;

        if mistakes.is_empty() {
            return Ok(String::new());
        }

        info!("📚 [LEARNING] Found {} relevant past mistakes", mistakes.len());

        // Format mistakes for inclusion in system prompt
        let mut context = String::from("\n\n## LEARNED FROM PAST MISTAKES:\n");
        context.push_str("Apply these lessons to avoid repeating errors:\n\n");

        for (i, mistake) in mistakes.iter().enumerate() {
            let severity_icon = match mistake.severity {
                Severity::Critical => "🚨",
                Severity::Major => "⚠️",
                Severity::Minor => "💡",
            };

            context.push_str(&format!(
                "{} {}. [{}] {}\n   Prevention: {}\n\n",
                severity_icon,
                i + 1,
                mistake.mistake_type,
                mistake.description,
                mistake.prevention_strategy
            ));
        }

        Ok(context)
    }

    /// Build a correction prompt for retrying a failed task
    ///
    /// Includes detailed information about what went wrong and how to fix it
    pub fn build_correction_prompt(
        &self,
        original_task: &str,
        validation: &ValidationResult,
        spec: &TaskSpecification,
        attempt_number: u32,
    ) -> String {
        let mut prompt = String::new();

        prompt.push_str(&format!(
            "# RETRY ATTEMPT {} - Previous attempt failed validation\n\n",
            attempt_number
        ));

        prompt.push_str("## Original Task:\n");
        prompt.push_str(original_task);
        prompt.push_str("\n\n");

        // Add failure summary
        prompt.push_str("## What Went Wrong:\n");
        prompt.push_str(&validation.summary);
        prompt.push_str("\n\n");

        // Detail each failure
        if !validation.missing_elements.is_empty() {
            prompt.push_str("### Missing Elements:\n");
            for missing in &validation.missing_elements {
                prompt.push_str(&format!(
                    "- {} ({}): {}\n",
                    missing.element, missing.category, missing.details
                ));
            }
            prompt.push_str("\n");
        }

        // Add failed requirements
        let failed_reqs: Vec<_> = validation.requirement_results.iter().filter(|r| !r.passed).collect();
        if !failed_reqs.is_empty() {
            prompt.push_str("### Failed Requirements:\n");
            for req in failed_reqs {
                prompt.push_str(&format!(
                    "- {}: {}\n  Expected: {:?}, Got: {:?}\n",
                    req.requirement,
                    req.explanation,
                    req.expected_value,
                    req.actual_value
                ));
            }
            prompt.push_str("\n");
        }

        // Add specific correction instructions
        prompt.push_str("## How to Fix:\n");

        // Generate correction instructions based on spec
        for req in &spec.numeric_requirements {
            let comparison_str = match req.comparison {
                super::types::ComparisonOp::Exactly => "exactly",
                super::types::ComparisonOp::AtLeast => "at least",
                super::types::ComparisonOp::AtMost => "at most",
            };
            prompt.push_str(&format!(
                "- Ensure you have {} {} {}\n",
                comparison_str, req.expected_count, req.entity
            ));
        }

        for output in &spec.expected_outputs {
            if output.required {
                prompt.push_str(&format!(
                    "- Create the required output: {} ({:?})\n",
                    output.name, output.output_type
                ));
            }
        }

        prompt.push_str("\n## Important:\n");
        prompt.push_str("- Complete ALL requirements before finishing\n");
        prompt.push_str("- Verify each requirement is met before concluding\n");
        prompt.push_str("- Do not skip any steps that were missed before\n");

        debug!("[LEARNING] Built correction prompt for attempt {}", attempt_number);

        prompt
    }

    /// Build a concise mistake summary for token-constrained contexts
    pub fn build_concise_context(
        &self,
        keywords: &[String],
        fingerprint: &str,
        max_mistakes: usize,
    ) -> Result<String, AgentError> {
        let mistakes = self.brain.recall_relevant_mistakes(keywords, fingerprint, max_mistakes)?;

        if mistakes.is_empty() {
            return Ok(String::new());
        }

        let mut context = String::from("AVOID: ");
        let summaries: Vec<String> = mistakes
            .iter()
            .take(3)
            .map(|m| m.description.clone())
            .collect();

        context.push_str(&summaries.join("; "));

        Ok(context)
    }

    /// Format a single mistake for display
    pub fn format_mistake(&self, mistake: &MistakeNode) -> String {
        format!(
            "[{}] {}\n  Type: {}\n  Deviation: {}\n  Prevention: {}\n  Corrected: {}",
            mistake.severity,
            mistake.description,
            mistake.mistake_type,
            mistake.deviation_details,
            mistake.prevention_strategy,
            if mistake.was_corrected { "Yes" } else { "No" }
        )
    }

    /// Get all uncorrected mistakes related to a task type
    pub fn get_active_mistakes(&self, fingerprint: &str) -> Result<Vec<MistakeNode>, AgentError> {
        self.brain.get_uncorrected_mistakes_for_fingerprint(fingerprint)
    }

    /// Mark mistakes as corrected after successful retry
    pub fn mark_corrected(
        &self,
        mistakes: &[MistakeNode],
        correcting_task_id: &str,
    ) -> Result<(), AgentError> {
        for mistake in mistakes {
            self.brain.mark_mistake_corrected(&mistake.id, correcting_task_id)?;
        }

        if !mistakes.is_empty() {
            info!("✅ [LEARNING] Marked {} mistakes as corrected by task {}",
                mistakes.len(), &correcting_task_id[..8]);
        }

        Ok(())
    }
}

/// Extract keywords from a description for matching
fn extract_keywords(description: &str) -> Vec<String> {
    let stop_words = [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "must", "shall", "can", "need", "to", "of",
        "in", "for", "on", "with", "at", "by", "from", "as", "into", "and",
        "or", "but", "if", "then", "than", "so", "that", "this", "these",
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

/// Builder for constructing mistake-aware prompts
pub struct PromptBuilder {
    base_prompt: String,
    mistake_context: Option<String>,
    correction_context: Option<String>,
}

impl PromptBuilder {
    pub fn new(base_prompt: impl Into<String>) -> Self {
        Self {
            base_prompt: base_prompt.into(),
            mistake_context: None,
            correction_context: None,
        }
    }

    /// Add mistake context to the prompt
    pub fn with_mistakes(mut self, context: String) -> Self {
        if !context.is_empty() {
            self.mistake_context = Some(context);
        }
        self
    }

    /// Add correction context for retries
    pub fn with_correction(mut self, context: String) -> Self {
        if !context.is_empty() {
            self.correction_context = Some(context);
        }
        self
    }

    /// Build the final prompt
    pub fn build(self) -> String {
        let mut prompt = self.base_prompt;

        if let Some(mistakes) = self.mistake_context {
            prompt.push_str(&mistakes);
        }

        if let Some(correction) = self.correction_context {
            prompt.push_str("\n\n");
            prompt.push_str(&correction);
        }

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_keywords() {
        let desc = "Search 3 e-commerce sites for headphones";
        let keywords = extract_keywords(desc);
        assert!(keywords.contains(&"search".to_string()));
        assert!(keywords.contains(&"headphones".to_string()));
        assert!(!keywords.contains(&"for".to_string())); // stop word
    }

    #[test]
    fn test_prompt_builder() {
        let prompt = PromptBuilder::new("Base prompt")
            .with_mistakes("Mistake context".to_string())
            .with_correction("Correction context".to_string())
            .build();

        assert!(prompt.contains("Base prompt"));
        assert!(prompt.contains("Mistake context"));
        assert!(prompt.contains("Correction context"));
    }
}
