//! Mistake Storage and Recall - Graph operations for the self-improvement system.
//!
//! Provides:
//! - Storing mistakes in the graph database
//! - Recalling relevant mistakes based on task similarity
//! - Marking mistakes as corrected
//! - Converting validation failures to mistakes

use chrono::Utc;
use tracing::{debug, info};

use crate::types::{
    AgentError, MemoryNode, MemoryType, MistakeNode, MistakeType, Severity,
};
use crate::orchestrator::types::{
    ValidationResult, TaskSpecification,
};
use super::GraphBrain;

impl GraphBrain {
    // ═══════════════════════════════════════════════════════════════════
    // MISTAKE STORAGE
    // ═══════════════════════════════════════════════════════════════════

    /// Record a mistake in the graph database
    pub fn record_mistake(&self, mistake: &MistakeNode) -> Result<String, AgentError> {
        let keywords_str = mistake.keywords.join(",");

        let query = format!(
            r#"CREATE (:Mistake {{
                id: '{}',
                mistake_type: '{}',
                description: '{}',
                severity: '{}',
                deviation_details: '{}',
                prevention_strategy: '{}',
                keywords: '{}',
                task_fingerprint: '{}',
                was_corrected: {},
                source_task_id: '{}',
                corrected_by_task_id: '{}',
                created_at: '{}'
            }})"#,
            escape_string(&mistake.id),
            mistake.mistake_type,
            escape_string(&mistake.description),
            mistake.severity,
            escape_string(&mistake.deviation_details),
            escape_string(&mistake.prevention_strategy),
            escape_string(&keywords_str),
            escape_string(&mistake.task_fingerprint),
            mistake.was_corrected,
            escape_string(&mistake.source_task_id),
            mistake.corrected_by_task_id.as_deref().unwrap_or(""),
            mistake.created_at.to_rfc3339()
        );

        self.execute(&query)?;

        // Also store as a Memory for cross-system recall
        let memory = mistake.to_memory_node();
        self.remember_with_dedup(&memory, &mistake.keywords)?;

        info!("🚨 [MISTAKE] Recorded mistake: {} ({})", &mistake.id[..8], mistake.mistake_type);
        Ok(mistake.id.clone())
    }

    /// Mark a mistake as corrected
    pub fn mark_mistake_corrected(&self, mistake_id: &str, correcting_task_id: &str) -> Result<(), AgentError> {
        let query = format!(
            "MATCH (m:Mistake {{id: '{}'}}) SET m.was_corrected = true, m.corrected_by_task_id = '{}'",
            escape_string(mistake_id),
            escape_string(correcting_task_id)
        );

        self.execute(&query)?;
        info!("✅ [MISTAKE] Marked mistake {} as corrected by task {}", &mistake_id[..8], &correcting_task_id[..8]);
        Ok(())
    }

    /// Link a task to a mistake it caused (CAUSED relationship)
    pub fn link_caused(&self, task_id: &str, mistake_id: &str) -> Result<(), AgentError> {
        debug!("Linking task {} -> CAUSED -> mistake {}", task_id, mistake_id);
        Ok(())
    }

    /// Link a mistake to the task that corrected it (CORRECTED_BY relationship)
    pub fn link_corrected_by(&self, mistake_id: &str, task_id: &str) -> Result<(), AgentError> {
        debug!("Linking mistake {} -> CORRECTED_BY -> task {}", mistake_id, task_id);
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // MISTAKE RECALL
    // ═══════════════════════════════════════════════════════════════════

    /// Recall mistakes relevant to a task based on keywords and fingerprint
    pub fn recall_relevant_mistakes(
        &self,
        keywords: &[String],
        fingerprint: &str,
        limit: usize,
    ) -> Result<Vec<MistakeNode>, AgentError> {
        info!("🔍 [MISTAKE] Recalling mistakes for fingerprint: {}, keywords: {:?}",
            if fingerprint.len() > 8 { &fingerprint[..8] } else { fingerprint },
            keywords);

        self.with_connection(|conn| {
            // Query all uncorrected mistakes (prioritize) and corrected ones
            let query = "MATCH (m:Mistake) RETURN m.id, m.mistake_type, m.description, m.severity, m.deviation_details, m.prevention_strategy, m.keywords, m.task_fingerprint, m.was_corrected, m.source_task_id, m.corrected_by_task_id, m.created_at ORDER BY m.was_corrected ASC, m.created_at DESC";

            let result = conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut mistakes: Vec<(MistakeNode, f64)> = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 12 {
                    let mistake_keywords: Vec<String> = parts[6]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    let mistake = MistakeNode {
                        id: parts[0].to_string(),
                        mistake_type: parts[1].parse().unwrap_or(MistakeType::Other),
                        description: parts[2].to_string(),
                        severity: parts[3].parse().unwrap_or(Severity::Major),
                        deviation_details: parts[4].to_string(),
                        prevention_strategy: parts[5].to_string(),
                        keywords: mistake_keywords,
                        task_fingerprint: parts[7].to_string(),
                        was_corrected: parts[8].parse().unwrap_or(false),
                        source_task_id: parts[9].to_string(),
                        corrected_by_task_id: if parts[10].is_empty() { None } else { Some(parts[10].to_string()) },
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[11])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };

                    // Calculate relevance score
                    let score = mistake.relevance_score(keywords, fingerprint);
                    if score > 0.1 {
                        mistakes.push((mistake, score));
                    }
                }
            }

            // Sort by score and limit
            mistakes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            mistakes.truncate(limit);

            let result: Vec<MistakeNode> = mistakes.into_iter().map(|(m, _)| m).collect();

            if result.is_empty() {
                debug!("🔍 [MISTAKE] No relevant mistakes found");
            } else {
                info!("🔍 [MISTAKE] Found {} relevant mistakes:", result.len());
                for m in &result {
                    info!("🔍 [MISTAKE]   🚨 [{}] \"{}\"", m.mistake_type,
                        if m.description.len() > 40 { format!("{}...", &m.description[..40]) } else { m.description.clone() });
                }
            }

            Ok(result)
        })
    }

    /// Recall mistakes as Memory nodes for inclusion in system prompt
    pub fn recall_mistake_memories(&self, query: &str, limit: usize) -> Result<Vec<MemoryNode>, AgentError> {
        let scored = self.smart_recall(query, limit * 2)?;

        // Filter to only Mistake type memories
        let mistakes: Vec<MemoryNode> = scored
            .into_iter()
            .filter(|(m, _)| m.memory_type == MemoryType::Mistake)
            .take(limit)
            .map(|(m, _)| m)
            .collect();

        Ok(mistakes)
    }

    /// Get a specific mistake by ID
    pub fn get_mistake(&self, mistake_id: &str) -> Result<Option<MistakeNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (m:Mistake {{id: '{}'}}) RETURN m.id, m.mistake_type, m.description, m.severity, m.deviation_details, m.prevention_strategy, m.keywords, m.task_fingerprint, m.was_corrected, m.source_task_id, m.corrected_by_task_id, m.created_at LIMIT 1",
                escape_string(mistake_id)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let result_str = format!("{}", result);
            if let Some(line) = result_str.lines().nth(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 12 {
                    let mistake_keywords: Vec<String> = parts[6]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    let mistake = MistakeNode {
                        id: parts[0].to_string(),
                        mistake_type: parts[1].parse().unwrap_or(MistakeType::Other),
                        description: parts[2].to_string(),
                        severity: parts[3].parse().unwrap_or(Severity::Major),
                        deviation_details: parts[4].to_string(),
                        prevention_strategy: parts[5].to_string(),
                        keywords: mistake_keywords,
                        task_fingerprint: parts[7].to_string(),
                        was_corrected: parts[8].parse().unwrap_or(false),
                        source_task_id: parts[9].to_string(),
                        corrected_by_task_id: if parts[10].is_empty() { None } else { Some(parts[10].to_string()) },
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[11])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    return Ok(Some(mistake));
                }
            }

            Ok(None)
        })
    }

    /// Get all uncorrected mistakes for a task fingerprint
    pub fn get_uncorrected_mistakes_for_fingerprint(&self, fingerprint: &str) -> Result<Vec<MistakeNode>, AgentError> {
        self.with_connection(|conn| {
            let query = format!(
                "MATCH (m:Mistake {{task_fingerprint: '{}'}}) WHERE m.was_corrected = false RETURN m.id, m.mistake_type, m.description, m.severity, m.deviation_details, m.prevention_strategy, m.keywords, m.task_fingerprint, m.was_corrected, m.source_task_id, m.corrected_by_task_id, m.created_at ORDER BY m.created_at DESC",
                escape_string(fingerprint)
            );

            let result = conn.query(&query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut mistakes = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 12 {
                    let mistake_keywords: Vec<String> = parts[6]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    let mistake = MistakeNode {
                        id: parts[0].to_string(),
                        mistake_type: parts[1].parse().unwrap_or(MistakeType::Other),
                        description: parts[2].to_string(),
                        severity: parts[3].parse().unwrap_or(Severity::Major),
                        deviation_details: parts[4].to_string(),
                        prevention_strategy: parts[5].to_string(),
                        keywords: mistake_keywords,
                        task_fingerprint: parts[7].to_string(),
                        was_corrected: parts[8].parse().unwrap_or(false),
                        source_task_id: parts[9].to_string(),
                        corrected_by_task_id: if parts[10].is_empty() { None } else { Some(parts[10].to_string()) },
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[11])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    mistakes.push(mistake);
                }
            }

            Ok(mistakes)
        })
    }

    /// Get all recorded mistakes (for debugging/inspection)
    pub fn get_all_mistakes(&self) -> Result<Vec<MistakeNode>, AgentError> {
        self.with_connection(|conn| {
            let query = "MATCH (m:Mistake) RETURN m.id, m.mistake_type, m.description, m.severity, m.deviation_details, m.prevention_strategy, m.keywords, m.task_fingerprint, m.was_corrected, m.source_task_id, m.corrected_by_task_id, m.created_at ORDER BY m.created_at DESC LIMIT 50";

            let result = conn.query(query).map_err(|e| {
                AgentError::ConfigError(format!("Query error: {}", e))
            })?;

            let mut mistakes = Vec::new();
            let result_str = format!("{}", result);

            for line in result_str.lines().skip(1) {
                let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
                if parts.len() >= 12 {
                    let mistake_keywords: Vec<String> = parts[6]
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    let mistake = MistakeNode {
                        id: parts[0].to_string(),
                        mistake_type: parts[1].parse().unwrap_or(MistakeType::Other),
                        description: parts[2].to_string(),
                        severity: parts[3].parse().unwrap_or(Severity::Major),
                        deviation_details: parts[4].to_string(),
                        prevention_strategy: parts[5].to_string(),
                        keywords: mistake_keywords,
                        task_fingerprint: parts[7].to_string(),
                        was_corrected: parts[8].parse().unwrap_or(false),
                        source_task_id: parts[9].to_string(),
                        corrected_by_task_id: if parts[10].is_empty() { None } else { Some(parts[10].to_string()) },
                        created_at: chrono::DateTime::parse_from_rfc3339(parts[11])
                            .map(|t| t.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    };
                    mistakes.push(mistake);
                }
            }

            info!("📚 [MISTAKE] Retrieved {} total mistakes", mistakes.len());
            Ok(mistakes)
        })
    }
}

// ═══════════════════════════════════════════════════════════════════
// MISTAKE EXTRACTION
// ═══════════════════════════════════════════════════════════════════

/// Extract mistakes from validation failures
pub struct MistakeExtractor;

impl MistakeExtractor {
    /// Convert validation result failures to mistake nodes
    pub fn extract_mistakes(
        validation: &ValidationResult,
        spec: &TaskSpecification,
        source_task_id: &str,
    ) -> Vec<MistakeNode> {
        let mut mistakes = Vec::new();

        // Process missing elements
        for missing in &validation.missing_elements {
            let (mistake_type, severity) = categorize_missing(&missing.category);

            let prevention = generate_prevention_strategy(&missing.element, &missing.category);

            let mut mistake = MistakeNode::new(
                mistake_type,
                missing.details.clone(),
                severity,
                format!("Missing: {}", missing.element),
                prevention,
                source_task_id.to_string(),
            );

            // Add keywords from spec
            mistake = mistake.with_keywords(spec.keywords.clone());
            mistake = mistake.with_fingerprint(spec.fingerprint());

            mistakes.push(mistake);
        }

        // Process failed requirements
        for req_result in &validation.requirement_results {
            if !req_result.passed {
                let mistake = MistakeNode::new(
                    MistakeType::QualityIssue,
                    req_result.explanation.clone(),
                    Severity::Major,
                    format!(
                        "Expected: {:?}, Actual: {:?}",
                        req_result.expected_value,
                        req_result.actual_value
                    ),
                    format!("Verify {} before completing task", req_result.requirement),
                    source_task_id.to_string(),
                )
                .with_keywords(spec.keywords.clone())
                .with_fingerprint(spec.fingerprint());

                mistakes.push(mistake);
            }
        }

        // Process missing outputs
        for out_result in &validation.output_results {
            if !out_result.found && out_result.expected.required {
                let mistake = MistakeNode::new(
                    MistakeType::MissingOutput,
                    format!("Expected output '{}' was not created", out_result.expected.name),
                    Severity::Critical,
                    format!("Output '{}' ({:?}) not found", out_result.expected.name, out_result.expected.output_type),
                    format!("Ensure {} is created before task completion", out_result.expected.name),
                    source_task_id.to_string(),
                )
                .with_keywords(spec.keywords.clone())
                .with_fingerprint(spec.fingerprint());

                mistakes.push(mistake);
            }
        }

        mistakes
    }
}

/// Categorize a missing element into mistake type and severity
fn categorize_missing(category: &str) -> (MistakeType, Severity) {
    match category {
        "numeric" => (MistakeType::QuantityMismatch, Severity::Major),
        "output" => (MistakeType::MissingOutput, Severity::Critical),
        "qualitative" => (MistakeType::QualityIssue, Severity::Major),
        _ => (MistakeType::Other, Severity::Minor),
    }
}

/// Generate a prevention strategy based on the missing element
fn generate_prevention_strategy(element: &str, category: &str) -> String {
    match category {
        "numeric" => format!(
            "Before completing, explicitly count and verify that {} meets the required quantity",
            element
        ),
        "output" => format!(
            "Create {} as a final step and confirm its existence before marking task complete",
            element
        ),
        "qualitative" => format!(
            "Review output against requirement '{}' before completion",
            element
        ),
        _ => format!("Verify '{}' is properly handled", element),
    }
}

/// Escape single quotes for Cypher queries
fn escape_string(s: &str) -> String {
    s.replace('\'', "\\'").replace('\n', " ").replace('\r', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::types::MissingElement;

    #[test]
    fn test_extract_mistakes_from_validation() {
        let mut validation = ValidationResult::failure("Test failure".to_string());
        validation.missing_elements.push(MissingElement {
            element: "3 sites".to_string(),
            category: "numeric".to_string(),
            details: "Only found 2 sites instead of 3".to_string(),
        });

        let spec = TaskSpecification::new("Search 3 sites for products".to_string());

        let mistakes = MistakeExtractor::extract_mistakes(&validation, &spec, "task-123");

        assert_eq!(mistakes.len(), 1);
        assert_eq!(mistakes[0].mistake_type, MistakeType::QuantityMismatch);
        assert!(mistakes[0].description.contains("2 sites"));
    }

    #[test]
    fn test_categorize_missing() {
        let (t, s) = categorize_missing("numeric");
        assert_eq!(t, MistakeType::QuantityMismatch);
        assert_eq!(s, Severity::Major);

        let (t, s) = categorize_missing("output");
        assert_eq!(t, MistakeType::MissingOutput);
        assert_eq!(s, Severity::Critical);
    }
}
